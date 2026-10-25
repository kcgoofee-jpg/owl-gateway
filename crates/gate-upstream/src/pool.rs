//! 令牌池：每把上游 Token 的状态、后台增删改、选号（谁能最早开始 + 账号保护 + V5 预留）、
//! 排队中换号、健康 / 冷却标记。
//!
//! 池锁（`tokio::sync::Mutex`）就是旧版 `self._lock`：选号、换号、V5 预留与释放、后台增删改在它下面串行，
//! 会跨 await 持有（选号时要问账号保护和库里的 V5 计数）。出图节奏不在这里（见 `scheduler.rs`）。
//!
//! 选号（站长 10-22）：不再严格轮询，而是选「这张图能最早开始」的账号——
//! 开始时间 ≈ max(现在, 间隔到点) + 前面排着的张数 × 平均周期（[`Scheduler::est_start`]）；估算相同（差不到
//! [`TIE_SECS`]）时按轮询。排队期间别的账号空出来了（或者等了一半排队超时、别的账号明显更快），
//! 就把这张挪过去（[`Pool::try_move`]），资格规则与选号完全一样（启停、冷却、Anlas、V5 日限、账号保护）。
//!
//! 同一个账号上谁先轮到按公平排队（[`crate::pace::fair_cmp`]：今天出得少的先，等久了的按先来后到），
//! 估算开始时间、成员看到的位次都按这个顺序算（[`Token::ahead_of`] / [`Token::ahead_for`]）。

use crate::error::UpstreamError;
use crate::headers;
use crate::hooks::{Allowance, Hooks};
use crate::limiter::AdjustableLimiter;
use crate::pace::{Arrival, fair_cmp};
use crate::scheduler::Scheduler;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

/// 每把上游 Token 的可变状态（旧版 `TokenState` 的 slots）。
#[derive(Debug, Clone)]
pub struct TokenFields {
    pub position: usize,
    pub v5_daily_limit: i64,
    pub allow_anlas: bool,
    pub pending_v5: i64,
    pub admin_enabled: bool,
    pub fails: i64,
    /// 墙钟时间：429 / 403 冷却到何时。
    pub blocked_until: f64,
    pub disabled: bool,
    pub last_ok: f64,
    /// 单调时钟（客户端启动以来的秒）：下一张图最早何时可以发。只由 `scheduler.rs` 改。
    pub image_next_at: f64,
    pub forbidden_streak: i64,
    pub forbidden_trips: i64,
    pub forbidden_last: f64,
    /// 墙钟时间：官方刚确认这把 Token 的 V5 免费额度见底，到此之前免费 V5 先派给别的 Token。
    pub v5_out_until: f64,
    /// 墙钟时间：上游刚对这把 Token 回了 402（Anlas 不足 / 需要付费），到此之前要花 Anlas 的图不派给它。
    pub anlas_out_until: f64,
    /// 墙钟时间：最近一次上游 429（后台显示用；0 = 没有）。
    pub last_429: f64,
    /// 正在等这把 Token 出图的请求（排队票号 + 公平信息，按登记顺序存；轮到谁见 [`fair_cmp`]）；开始时移除。
    pub waiting: Vec<(u64, Arrival)>,
    /// 实测出图耗时（秒，滑动平均；0 = 还没有样本）。
    pub gen_secs: f64,
    /// 单调时钟：最近一次开始出图的时刻。
    pub last_start: f64,
    /// 今天（服务日 `locked_day`）上游回「并发锁」的次数（后台显示用，不算 429）。
    pub locked_today: i64,
    pub locked_day: String,
}

/// 要跨重启保留的冷却状态（审查 2026-10-17 M-4）：不然一次部署 / 重启就会立刻去试探刚被
/// 403 / 429 隔离的账号，403 退避档位也从 5 分钟重新算。存库见 gate-server `hooks.rs`。
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Cooldown {
    pub blocked_until: f64,
    pub forbidden_trips: i64,
    pub forbidden_last: f64,
}

/// 恢复时只认 1 小时内的冷却（与全站 `image_cooldown_until` 一致）和 24 小时内的 403 记录
/// （24 小时没有 403 时退避档位本来就归零，见 [`Token::mark_forbidden`]）。
const RESTORE_BLOCK_MAX: f64 = 3600.0;
/// V5 额度见底的 Token 多久后再派免费 V5 去试探（额度按小时缓慢恢复）。
const V5_OUT_SECS: f64 = 300.0;
/// 上游回 402 后，这把 Token 多久内不接要花 Anlas 的图（之后再试；余额另按订阅读数判断）。
const ANLAS_OUT_SECS: f64 = 600.0;
const RESTORE_FORBIDDEN_MAX: f64 = 86400.0;
/// 两个账号的估算开始时间差不到这么多秒算「一样快」，按轮询。
pub const TIE_SECS: f64 = 0.25;
/// 排队中换号：别的账号要比现在这个早这么多秒开始才值得挪（等了一半排队超时以后）。
pub const MOVE_MARGIN: f64 = 2.0;
/// 排队中换号：现在这个账号这么多秒内就能开始，就不挪了。
const MOVE_SOON: f64 = 1.0;
/// 「空出来了」：估算开始时间不超过这么多秒。
const FREE_SECS: f64 = 0.5;
/// 有更早来的请求也能去那个空账号时，先让它去；空了这么久它还没去，就不再让。
pub const MOVE_YIELD_SECS: f64 = 1.0;

impl Cooldown {
    /// 还有没有需要保留的内容（过期的条目存库时丢掉）。
    pub fn live(&self, now: f64) -> bool {
        self.blocked_until > now
            || (self.forbidden_last > 0.0 && now - self.forbidden_last <= RESTORE_FORBIDDEN_MAX)
    }

    /// 合并同一账号的两次记录：冷却取更晚的（宁长勿短），403 档位以最近一次 403 为准。
    pub fn merge(self, other: Cooldown) -> Cooldown {
        let newer = if other.forbidden_last >= self.forbidden_last {
            other
        } else {
            self
        };
        Cooldown {
            blocked_until: self.blocked_until.max(other.blocked_until),
            forbidden_trips: newer.forbidden_trips,
            forbidden_last: newer.forbidden_last,
        }
    }
}

/// 存库格式：`{"<token_id>": [blocked_until, forbidden_trips, forbidden_last], ...}`。
pub fn encode_cooldowns(map: &HashMap<String, Cooldown>) -> String {
    let obj: serde_json::Map<String, serde_json::Value> = map
        .iter()
        .map(|(id, c)| {
            (
                id.clone(),
                serde_json::json!([c.blocked_until, c.forbidden_trips, c.forbidden_last]),
            )
        })
        .collect();
    serde_json::Value::Object(obj).to_string()
}

/// 解析存库格式；整体或单条不合法都当作没有（只会让冷却变短，不会让请求出错）。
/// 10-22～10-23 的版本还存了第 4、5 项（出图间隔 AIMD 的状态，已删除）：读到也忽略。
pub fn decode_cooldowns(raw: Option<&str>) -> HashMap<String, Cooldown> {
    let Some(serde_json::Value::Object(obj)) = raw.and_then(|s| serde_json::from_str(s).ok())
    else {
        return HashMap::new();
    };
    obj.into_iter()
        .filter_map(|(id, v)| {
            let a = v.as_array()?;
            let c = Cooldown {
                blocked_until: a.first()?.as_f64()?,
                forbidden_trips: a.get(1)?.as_i64()?,
                forbidden_last: a.get(2)?.as_f64()?,
            };
            (c.blocked_until.is_finite() && c.forbidden_last.is_finite()).then_some((id, c))
        })
        .collect()
}

pub struct Token {
    pub token: String,
    pub token_id: String,
    profile_index: usize,
    pub image_slots: AdjustableLimiter,
    /// 与「停用 Token」串行：之后发出的请求一定能看到新的启停状态（请求本身不持锁）。
    pub(crate) dispatch_lock: tokio::sync::Mutex<()>,
    fields: Mutex<TokenFields>,
}

impl Token {
    fn new(token: String, index: usize, v5_daily_limit: i64, allow_anlas: bool) -> Self {
        Self {
            token_id: headers::token_id(&token),
            token,
            profile_index: index % headers::POOL.len(),
            image_slots: AdjustableLimiter::new(1),
            dispatch_lock: tokio::sync::Mutex::new(()),
            fields: Mutex::new(TokenFields {
                position: index + 1,
                v5_daily_limit: v5_daily_limit.max(0),
                allow_anlas,
                pending_v5: 0,
                admin_enabled: true,
                fails: 0,
                blocked_until: 0.0,
                disabled: false,
                last_ok: 0.0,
                image_next_at: 0.0,
                forbidden_streak: 0,
                forbidden_trips: 0,
                forbidden_last: 0.0,
                v5_out_until: 0.0,
                anlas_out_until: 0.0,
                last_429: 0.0,
                waiting: Vec::new(),
                gen_secs: 0.0,
                last_start: 0.0,
                locked_today: 0,
                locked_day: String::new(),
            }),
        }
    }

    pub fn fields(&self) -> TokenFields {
        self.fields.lock().expect("token").clone()
    }

    pub(crate) fn with<R>(&self, f: impl FnOnce(&mut TokenFields) -> R) -> R {
        f(&mut self.fields.lock().expect("token"))
    }

    /// `admin_enabled and not disabled and now >= blocked_until`
    pub fn usable(&self, now: f64) -> bool {
        self.with(|f| f.admin_enabled && !f.disabled && now >= f.blocked_until)
    }

    pub fn cooldown(&self) -> Cooldown {
        self.with(|f| Cooldown {
            blocked_until: f.blocked_until,
            forbidden_trips: f.forbidden_trips,
            forbidden_last: f.forbidden_last,
        })
    }

    /// 启动时恢复存库的冷却：未来 1 小时内的冷却照旧生效，24 小时内的 403 退避档位接着算。
    /// 只会让账号更保守（冷却取更晚的），不会提前放行。
    pub fn restore_cooldown(&self, c: Cooldown, now: f64) {
        self.with(|f| {
            if c.blocked_until > now && c.blocked_until <= now + RESTORE_BLOCK_MAX {
                f.blocked_until = f.blocked_until.max(c.blocked_until);
            }
            if c.forbidden_last > 0.0
                && c.forbidden_last <= now
                && now - c.forbidden_last <= RESTORE_FORBIDDEN_MAX
            {
                f.forbidden_trips = c.forbidden_trips.clamp(0, 30);
                f.forbidden_last = c.forbidden_last;
            }
        });
    }

    /// 启动时恢复明确由上游 401 标记的凭据隔离；这与后台人工启停分开保存。
    pub fn restore_unauthorized(&self) {
        self.with(|f| f.disabled = true);
    }

    /// 直接设定实测出图耗时（秒；平时由每次出图的滑动平均更新）。测试和估算演示用。
    pub fn set_gen_secs(&self, secs: f64) {
        self.with(|f| f.gen_secs = secs.max(0.0));
    }

    /// 排在这把 Token 上等出图的请求数。
    pub fn waiting_len(&self) -> usize {
        self.with(|f| f.waiting.len())
    }

    /// 票号 `id` 前面还有几张会先轮到（公平排队的顺序，`now` 是单调秒；不在队里时按队尾算）。
    pub(crate) fn ahead_of(&self, id: u64, now: f64) -> usize {
        self.with(|f| {
            let Some((_, me)) = f.waiting.iter().find(|(w, _)| *w == id) else {
                return f.waiting.len();
            };
            f.waiting
                .iter()
                .filter(|(w, a)| fair_cmp((a, *w), (me, id), now).is_lt())
                .count()
        })
    }

    /// 现在来一张 `arrival` 这样的图，前面会有几张先轮到。
    pub(crate) fn ahead_for(&self, arrival: &Arrival, now: f64) -> usize {
        self.with(|f| {
            f.waiting
                .iter()
                .filter(|(w, a)| fair_cmp((a, *w), (arrival, u64::MAX), now).is_lt())
                .count()
        })
    }

    /// 从等待队列里移除这张票，并叫醒在等这个账号的（排在后面的位次变了）。
    pub(crate) fn leave_queue(&self, id: u64) {
        let removed = self.with(|f| {
            let before = f.waiting.len();
            f.waiting.retain(|(w, _)| *w != id);
            f.waiting.len() != before
        });
        if removed {
            self.image_slots.notify().notify_waiters();
        }
    }

    /// 上游回了一次「并发锁」（按服务日计数，后台显示）。
    pub(crate) fn note_locked(&self, hooks: &dyn Hooks) {
        let day = hooks.day();
        self.with(|f| {
            if f.locked_day != day {
                f.locked_day = day;
                f.locked_today = 0;
            }
            f.locked_today += 1;
        });
    }

    /// 今天上游回「并发锁」的次数。
    pub fn locked_today(&self, day: &str) -> i64 {
        self.with(|f| {
            if f.locked_day == day {
                f.locked_today
            } else {
                0
            }
        })
    }

    /// 只看内存状态的资格（不查库）：可用、能付 Anlas（要的话）、免费 V5 没被确认见底（要的话）。
    fn quick_eligible(
        &self,
        now: f64,
        requires_anlas: bool,
        v5_free: bool,
        allowance: Option<&dyn Allowance>,
    ) -> bool {
        self.usable(now)
            && (!requires_anlas || self.can_pay_anlas(now, allowance))
            && (!v5_free || now >= self.with(|f| f.v5_out_until))
    }

    /// 这把 Token 现在能不能出要花 Anlas 的图：后台允许、最近没被上游回 402、
    /// 最近读到的余额（读得到的话）够这张图（见 [`Allowance::anlas_short`]）。
    pub fn can_pay_anlas(&self, now: f64, allowance: Option<&dyn Allowance>) -> bool {
        self.with(|f| f.allow_anlas && now >= f.anlas_out_until)
            && !allowance.is_some_and(|a| a.anlas_short(&self.token_id))
    }

    /// 上游回 402（Anlas 不足 / 需要付费）：这把 Token 一段时间内不接要花 Anlas 的图。
    pub(crate) fn mark_anlas_out(&self, hooks: &dyn Hooks) {
        let until = hooks.now() + ANLAS_OUT_SECS;
        self.with(|f| f.anlas_out_until = f.anlas_out_until.max(until));
    }

    /// 这把 Token 用的指纹序号（取 Token 加入时的位置；删除 / 重排后位置会变，指纹不变）。
    pub fn profile_index(&self) -> usize {
        self.profile_index
    }

    // ---------------- 健康标记 ----------------

    pub(crate) fn mark_rate_limited(&self, hooks: &dyn Hooks, retry_after: f64) {
        let until = hooks.now() + gate_core::upstream_policy::clamp_retry_after_f64(retry_after);
        self.with(|f| {
            f.blocked_until = until;
            f.fails += 1;
        });
        hooks.cooldown_changed(&self.token_id, self.cooldown());
    }

    /// 连续 3 次 403（中间没有成功）→ 冷却并告警；冷却按轮次 5 → 15 → 45 → 60 分钟退避，
    /// 退避档位只在 24 小时内没有 403 时归零（间歇性 403 不会被一次成功打回 5 分钟）。
    pub(crate) fn mark_forbidden(&self, hooks: &dyn Hooks) {
        let now = hooks.now();
        let tripped = self.with(|f| {
            f.fails += 1;
            if f.forbidden_last != 0.0 && now - f.forbidden_last > 86400.0 {
                f.forbidden_trips = 0;
            }
            f.forbidden_last = now;
            f.forbidden_streak += 1;
            if f.forbidden_streak >= 3 {
                let minutes =
                    60_i64.min(5 * 3_i64.saturating_pow(f.forbidden_trips.clamp(0, 30) as u32));
                f.forbidden_trips += 1;
                f.forbidden_streak = 0;
                f.blocked_until = f.blocked_until.max(now + minutes as f64 * 60.0);
                Some((minutes, f.forbidden_trips))
            } else {
                None
            }
        });
        hooks.cooldown_changed(&self.token_id, self.cooldown());
        if let Some((minutes, trips)) = tripped {
            hooks.event(
                "upstream_403_cooldown",
                &format!("NovelAI 连续返回 403，账号可能被上游封控。该 Token（{}）冷却 {minutes} 分钟后自动重试（第 {trips} 轮），请检查上游账号状态。", self.token_id),
                1800.0,
            );
        }
    }

    pub(crate) fn mark_unauthorized(&self) {
        self.with(|f| f.disabled = true);
    }

    pub(crate) fn mark_ok(&self, hooks: &dyn Hooks) {
        let now = hooks.now();
        self.with(|f| {
            f.fails = 0;
            f.forbidden_streak = 0;
            f.last_ok = now;
            // forbidden_trips 故意不清零（见 mark_forbidden）
        });
    }
}

/// 统一处理已使用凭据收到的 401：立即隔离运行态，并等待实现层持久化指纹。
pub(crate) async fn isolate_unauthorized(hooks: &dyn Hooks, token: &Token) {
    token.mark_unauthorized();
    if let Err(e) = hooks.unauthorized(&token.token_id).await {
        tracing::error!(token_id = %token.token_id, error = %e, "上游 401 凭据隔离存库失败；本次运行仍已隔离该账号");
    }
    event_401(hooks, &token.token_id);
}

/// 402 / 403：提醒站长检查上游账号。
pub(crate) fn warn_account(hooks: &dyn Hooks, status: u16) {
    if status == 402 || status == 403 {
        let reason = if status == 402 {
            "需要付费 / 订阅或 Anlas 不足"
        } else {
            "拒绝访问，账号可能受限"
        };
        hooks.event(
            &format!("upstream_{status}"),
            &format!("NovelAI 返回 {status}（{reason}），请检查上游账号状态。"),
            1800.0,
        );
    }
}

pub(crate) fn event_401(hooks: &dyn Hooks, token_id: &str) {
    hooks.event(
        "upstream_401",
        &format!("NovelAI 对上游账号 {token_id} 返回 401，该账号凭据已隔离，请验证并更换该账号的 Token。其他账号状态未作判断。"),
        1800.0,
    );
}

struct PoolState {
    tokens: Vec<Arc<Token>>,
    rr: usize,
}

/// 排队票（只给要等出图间隔的请求）：登记在某把 Token 的 `waiting` 里，前面有几张决定估算的开始时间；
/// 轮到它开始（或换号、放弃、被取消）时 Drop 移除。
#[derive(Debug, Clone)]
struct WaiterMeta {
    token_id: String,
    requires_anlas: bool,
    v5_free: bool,
}

#[derive(Default)]
struct Waiters {
    next: u64,
    /// 票号 → 在等哪个账号、这张图的费用要求（换号时「先来的先换」要看）。
    metas: BTreeMap<u64, WaiterMeta>,
}

pub(crate) struct Ticket {
    token: Arc<Token>,
    id: u64,
    /// 公平排队的信息（换号时原样带到新账号）。
    arrival: Arrival,
    waiters: Arc<Mutex<Waiters>>,
}

impl Ticket {
    pub(crate) fn token(&self) -> &Arc<Token> {
        &self.token
    }

    pub(crate) fn id(&self) -> u64 {
        self.id
    }

    /// 这张图的费用要求变了（V5 确认成「按 Anlas 计」）。
    pub(crate) fn set_flags(&self, requires_anlas: bool, v5_free: bool) {
        if let Some(m) = self
            .waiters
            .lock()
            .expect("waiters")
            .metas
            .get_mut(&self.id)
        {
            m.requires_anlas = requires_anlas;
            m.v5_free = v5_free;
        }
    }
}

impl Drop for Ticket {
    fn drop(&mut self) {
        self.token.leave_queue(self.id);
        self.waiters.lock().expect("waiters").metas.remove(&self.id);
    }
}

pub(crate) struct Pool {
    state: tokio::sync::Mutex<PoolState>,
    waiters: Arc<Mutex<Waiters>>,
}

/// 排队中换号时调用方保存的状态：第一次看到某个账号空出来的时刻（给更早来的请求让一会儿）。
#[derive(Default)]
pub(crate) struct MoveWatch {
    seen: Option<(String, tokio::time::Instant)>,
}

impl Pool {
    /// `tokens`：(Token, 每日免费 V5 上限, 是否允许 Anlas)。
    pub(crate) fn new(tokens: Vec<(String, i64, bool)>) -> Self {
        let tokens = tokens
            .into_iter()
            .enumerate()
            .map(|(i, (t, v5, anlas))| Arc::new(Token::new(t, i, v5, anlas)))
            .collect();
        Self {
            state: tokio::sync::Mutex::new(PoolState { tokens, rr: 0 }),
            waiters: Arc::default(),
        }
    }

    /// 在 `token` 上登记一张排队票（调用方持池锁）。
    fn issue(
        &self,
        token: &Arc<Token>,
        requires_anlas: bool,
        v5_free: bool,
        arrival: Arrival,
    ) -> Ticket {
        let id = {
            let mut w = self.waiters.lock().expect("waiters");
            w.next += 1;
            let id = w.next;
            w.metas.insert(
                id,
                WaiterMeta {
                    token_id: token.token_id.clone(),
                    requires_anlas,
                    v5_free,
                },
            );
            id
        };
        token.with(|f| f.waiting.push((id, arrival)));
        // 新来的可能排到别人前面：叫醒在等的重新看位次
        token.image_slots.notify().notify_waiters();
        Ticket {
            token: token.clone(),
            id,
            arrival,
            waiters: self.waiters.clone(),
        }
    }

    /// 这张票还要等多少秒才能开始（估算）。
    pub(crate) fn eta(&self, sched: &Scheduler, hooks: &dyn Hooks, ticket: &Ticket) -> f64 {
        let t = &ticket.token;
        sched.est_start(t, hooks, t.ahead_of(ticket.id, sched.mono()))
    }

    pub(crate) async fn tokens(&self) -> Vec<Arc<Token>> {
        self.state.lock().await.tokens.clone()
    }

    pub(crate) async fn find(&self, token_id: &str) -> Option<Arc<Token>> {
        let pool = self.state.lock().await;
        pool.tokens.iter().find(|t| t.token_id == token_id).cloned()
    }

    // ---------------- 后台增删改 ----------------

    pub(crate) async fn apply_saved(
        &self,
        v5_limits: &HashMap<String, i64>,
        enabled: &HashMap<String, bool>,
    ) {
        let pool = self.state.lock().await;
        for t in &pool.tokens {
            t.with(|f| {
                if let Some(v) = v5_limits.get(&t.token_id) {
                    f.v5_daily_limit = *v;
                }
                if let Some(e) = enabled.get(&t.token_id) {
                    f.admin_enabled = *e;
                }
            });
        }
    }

    pub(crate) async fn restore_unauthorized(&self, token_ids: &std::collections::HashSet<String>) {
        let pool = self.state.lock().await;
        for token in &pool.tokens {
            if token_ids.contains(&token.token_id) {
                token.restore_unauthorized();
            }
        }
    }

    pub(crate) async fn add(
        &self,
        token: String,
        allow_anlas: bool,
        saved_v5: Option<i64>,
        saved_enabled: Option<bool>,
    ) -> Result<Arc<Token>, String> {
        let mut pool = self.state.lock().await;
        let new = Token::new(token, pool.tokens.len(), 0, allow_anlas);
        if pool.tokens.iter().any(|t| t.token_id == new.token_id) {
            return Err("这把 Token 已经在令牌池里".into());
        }
        new.with(|f| {
            if let Some(v) = saved_v5 {
                f.v5_daily_limit = v;
            }
            if let Some(e) = saved_enabled {
                f.admin_enabled = e;
            }
        });
        new.image_slots.resize(1);
        let new = Arc::new(new);
        pool.tokens.push(new.clone());
        renumber(&pool.tokens);
        Ok(new)
    }

    pub(crate) async fn replace(
        &self,
        token_id: &str,
        token: String,
    ) -> Result<(Arc<Token>, String), String> {
        let mut pool = self.state.lock().await;
        let Some(index) = pool.tokens.iter().position(|t| t.token_id == token_id) else {
            return Err("令牌不存在".into());
        };
        let old = pool.tokens[index].clone();
        let of = old.fields();
        let new = Token::new(token, index, of.v5_daily_limit, of.allow_anlas);
        if new.token_id != old.token_id && pool.tokens.iter().any(|t| t.token_id == new.token_id) {
            return Err("这把 Token 已经在令牌池的另一个位置".into());
        }
        new.with(|f| f.admin_enabled = of.admin_enabled);
        new.image_slots.resize(old.image_slots.limit());
        let new = Arc::new(new);
        pool.tokens[index] = new.clone();
        renumber(&pool.tokens);
        Ok((new, old.token_id.clone()))
    }

    pub(crate) async fn remove(&self, token_id: &str) -> Result<bool, String> {
        let mut pool = self.state.lock().await;
        if pool.tokens.len() <= 1 {
            return Err("至少需要保留一把上游 Token".into());
        }
        let before = pool.tokens.len();
        pool.tokens.retain(|t| t.token_id != token_id);
        if pool.tokens.len() == before {
            return Ok(false);
        }
        renumber(&pool.tokens);
        Ok(true)
    }

    pub(crate) async fn set_v5_daily_limit(&self, token_id: &str, limit: i64) -> bool {
        let pool = self.state.lock().await;
        match pool.tokens.iter().find(|t| t.token_id == token_id) {
            Some(t) => {
                t.with(|f| f.v5_daily_limit = limit);
                true
            }
            None => false,
        }
    }

    /// 停用 / 启用。停用会等这把 Token 手上的图跑完才返回（之后发出的请求一定能看到新状态）。
    pub(crate) async fn set_admin_enabled(&self, token_id: &str, enabled: bool) -> bool {
        let Some(t) = self.find(token_id).await else {
            return false;
        };
        {
            let _d = t.dispatch_lock.lock().await;
            let _p = self.state.lock().await;
            t.with(|f| f.admin_enabled = enabled);
        }
        if !enabled {
            t.image_slots.wait_idle().await;
        }
        true
    }

    // ---------------- 选号与预留 ----------------

    /// 选一把符合费用策略、这张图能最早开始的 Token；V5 免费额度在这里原子预留。
    /// 要花 Anlas 的图跳过不能付 Anlas 的 Token（见 [`Token::can_pay_anlas`]）。
    /// `image_job`（图片车道）时还要过账号保护，并在选中的 Token 上登记排队票（见 [`Ticket`]）；
    /// 估算开始时间按公平排队里这张会排在第几（`arrival`）。
    #[allow(clippy::too_many_arguments)] // 费用要求、车道、公平信息都只在选号这一层用
    pub(crate) async fn pick(
        &self,
        hooks: &dyn Hooks,
        sched: &Scheduler,
        requires_anlas: bool,
        v5_free: bool,
        image_job: bool,
        allowance: Option<&dyn Allowance>,
        arrival: Arrival,
    ) -> Result<Option<(Arc<Token>, Option<Ticket>)>, UpstreamError> {
        let mut pool = self.state.lock().await;
        let now = hooks.now();
        let mut usable: Vec<Arc<Token>> = pool
            .tokens
            .iter()
            .filter(|t| t.usable(now) && (!requires_anlas || t.can_pay_anlas(now, allowance)))
            .cloned()
            .collect();
        if usable.is_empty() {
            return Ok(None);
        }
        if image_job && hooks.has_guard() {
            let day = hooks.day();
            let mut reasons = Vec::new();
            let mut open = Vec::new();
            for t in usable {
                match hooks
                    .token_block_reason(&t.token_id, &day)
                    .await
                    .map_err(UpstreamError::internal)?
                {
                    Some(r) => reasons.push(r),
                    None => open.push(t),
                }
            }
            if open.is_empty() {
                let (text, code) = reasons.into_iter().next().unwrap_or_default();
                let mut e = UpstreamError::new(429, text);
                e.code = code;
                return Err(e);
            }
            usable = open;
        }
        // 从轮询指针的下一把开始看：估算一样快时先看到的赢（= 轮询）。跳过当日 V5 已用完的 Token；
        // 免费 V5 先避开官方额度刚见底的 Token（第一轮都不行才用它，那时这张图会按 Anlas 计）
        let n = usable.len();
        let start = (pool.rr + 1) % n;
        let mut chosen: Option<(f64, Arc<Token>)> = None;
        for round in 0..2 {
            for offset in 0..n {
                let candidate = &usable[(start + offset) % n];
                if v5_free && round == 0 && candidate.with(|f| now < f.v5_out_until) {
                    continue;
                }
                if v5_free && !under_v5_limit(hooks, candidate).await? {
                    continue;
                }
                let est = if image_job {
                    sched.est_start(
                        candidate,
                        hooks,
                        candidate.ahead_for(&arrival, sched.mono()),
                    )
                } else {
                    0.0
                };
                if chosen
                    .as_ref()
                    .is_none_or(|(best, _)| est < best - TIE_SECS)
                {
                    chosen = Some((est, candidate.clone()));
                }
                if !image_job {
                    break; // 不排队的请求只看轮询
                }
            }
            if chosen.is_some() || !v5_free {
                break;
            }
        }
        let Some((_, chosen)) = chosen else {
            return Ok(None);
        };
        pool.rr = usable
            .iter()
            .position(|t| Arc::ptr_eq(t, &chosen))
            .unwrap_or(0);
        if v5_free {
            chosen.with(|f| f.pending_v5 += 1);
        }
        let ticket = image_job.then(|| self.issue(&chosen, requires_anlas, v5_free, arrival));
        Ok(Some((chosen, ticket)))
    }

    /// 排队中换号：`ticket` 还在等它的账号时，看看别的账号能不能更早开始这张图。
    /// - 别的账号「空出来了」（估算马上能开始）就换；
    /// - `patient_over`（已经等了一半排队超时）时，别的账号比现在这个早 [`MOVE_MARGIN`] 秒以上开始也换；
    /// - 有更早来的请求也能去那个账号时先让它（最多让 [`MOVE_YIELD_SECS`] 秒），保持先来先出。
    ///
    /// 资格与 [`Pool::pick`] 一样（可用、Anlas、V5 见底与日限、账号保护）。换成功时在新账号上预留 V5（要的话）
    /// 并登记新票返回；旧票和旧账号的 V5 预留由调用方结算。
    #[allow(clippy::too_many_arguments)] // 与 pick 的参数一一对应
    pub(crate) async fn try_move(
        &self,
        hooks: &dyn Hooks,
        sched: &Scheduler,
        ticket: &Ticket,
        requires_anlas: bool,
        v5_free: bool,
        allowance: Option<&dyn Allowance>,
        patient_over: bool,
        watch: &mut MoveWatch,
    ) -> Result<Option<(Arc<Token>, Ticket)>, UpstreamError> {
        let pool = self.state.lock().await;
        let now = hooks.now();
        let mono = sched.mono();
        let mine = sched.est_start(&ticket.token, hooks, ticket.token.ahead_of(ticket.id, mono));
        if mine <= MOVE_SOON {
            watch.seen = None;
            return Ok(None);
        }
        // 先只看内存（便宜）：有没有值得换的账号，取最早能开始的
        let mut best: Option<(f64, Arc<Token>)> = None;
        for t in &pool.tokens {
            if Arc::ptr_eq(t, &ticket.token)
                || !t.quick_eligible(now, requires_anlas, v5_free, allowance)
            {
                continue;
            }
            let est = sched.est_start(t, hooks, t.ahead_for(&ticket.arrival, mono));
            let worth = est <= FREE_SECS || (patient_over && est + MOVE_MARGIN < mine);
            if worth && best.as_ref().is_none_or(|(b, _)| est < *b) {
                best = Some((est, t.clone()));
            }
        }
        let Some((_, target)) = best else {
            watch.seen = None;
            return Ok(None);
        };
        // 先来先出：更早排队、而且也能去那个账号的请求先去
        let first_seen = match &watch.seen {
            Some((id, at)) if *id == target.token_id => *at,
            _ => {
                let at = tokio::time::Instant::now();
                watch.seen = Some((target.token_id.clone(), at));
                at
            }
        };
        let older_fits = {
            let w = self.waiters.lock().expect("waiters");
            w.metas.range(..ticket.id).any(|(_, m)| {
                m.token_id != target.token_id
                    && target.quick_eligible(now, m.requires_anlas, m.v5_free, allowance)
            })
        };
        if older_fits && first_seen.elapsed().as_secs_f64() < MOVE_YIELD_SECS {
            return Ok(None);
        }
        // 再查库：账号保护、V5 日限
        if hooks.has_guard()
            && hooks
                .token_block_reason(&target.token_id, &hooks.day())
                .await
                .map_err(UpstreamError::internal)?
                .is_some()
        {
            return Ok(None);
        }
        if v5_free && !under_v5_limit(hooks, &target).await? {
            return Ok(None);
        }
        watch.seen = None;
        if v5_free {
            target.with(|f| f.pending_v5 += 1);
        }
        let new = self.issue(&target, requires_anlas, v5_free, ticket.arrival);
        Ok(Some((target, new)))
    }

    /// 除了这把，还有没有 Token 能免费出 V5（可用、且 [`v5_room`] 成立）。查计数失败当作没有。
    pub(crate) async fn other_v5_open(&self, hooks: &dyn Hooks, token: &Token) -> bool {
        let pool = self.state.lock().await;
        let now = hooks.now();
        for t in &pool.tokens {
            if !std::ptr::eq(t.as_ref(), token)
                && t.usable(now)
                && v5_room(hooks, t, now).await.unwrap_or(false)
            {
                return true;
            }
        }
        false
    }

    /// 全站还有没有上游账号能免费出 V5：后台启用、没失效的账号里有一把 [`v5_room`] 成立
    /// （与网关改派 / 以 `v5_exhausted` 拒绝用的是同一个判断）。正在 429 / 403 冷却的账号也算
    /// （那是限流，不是额度用完）；一把启用的账号都没有时返回 true（不是 V5 额度的问题）。
    pub(crate) async fn v5_free_open(&self, hooks: &dyn Hooks) -> Result<bool, UpstreamError> {
        let pool = self.state.lock().await;
        let now = hooks.now();
        let mut any = false;
        for t in &pool.tokens {
            if !t.with(|f| f.admin_enabled && !f.disabled) {
                continue;
            }
            any = true;
            if v5_room(hooks, t, now).await? {
                return Ok(true);
            }
        }
        Ok(!any)
    }

    /// 官方确认这把 Token 的免费 V5 见底（或免费 V5 被上游回了 402）：5 分钟内免费 V5 先派给别的 Token
    /// （之后再试探）。
    pub(crate) fn mark_v5_out(&self, hooks: &dyn Hooks, token: &Token) {
        let now = hooks.now();
        token.with(|f| f.v5_out_until = now + V5_OUT_SECS);
    }

    /// 只把成功完成的免费 V5 图计入该 Token 的日额度；无论成败都释放预留。
    pub(crate) async fn finish_v5_reservation(
        &self,
        hooks: &dyn Hooks,
        token: &Token,
        succeeded: bool,
        v5_free: bool,
    ) {
        if !v5_free {
            return;
        }
        let _pool = self.state.lock().await;
        if succeeded {
            let day = hooks.day();
            if let Err(e) = hooks.bump_upstream_v5(&token.token_id, &day).await {
                tracing::error!(error = %e, "记录上游 V5 计数失败");
            }
        }
        token.with(|f| f.pending_v5 = (f.pending_v5 - 1).max(0));
    }

    /// 没有可用 Token 时给成员的说明：先区分「冷却 / 停用」和「额度真的用完」。
    pub(crate) async fn unavailable(
        &self,
        hooks: &dyn Hooks,
        requires_anlas: bool,
        v5_free: bool,
    ) -> UpstreamError {
        let pool = self.state.lock().await;
        let now = hooks.now();
        let usable: Vec<_> = pool.tokens.iter().filter(|t| t.usable(now)).collect();
        if usable.is_empty() {
            let cooling: Vec<f64> = pool
                .tokens
                .iter()
                .filter_map(|t| {
                    t.with(|f| {
                        (f.admin_enabled && !f.disabled && now < f.blocked_until)
                            .then_some(f.blocked_until)
                    })
                })
                .collect();
            if !cooling.is_empty() {
                let earliest = cooling.into_iter().fold(f64::INFINITY, f64::min);
                let wait = ((earliest - hooks.now()) as i64).max(1);
                return UpstreamError::new(
                    503,
                    format!("本站 NovelAI 账号正在限流冷却，未扣费。约 {wait} 秒后恢复。"),
                );
            }
            return UpstreamError::new(503, "本站 NovelAI 账号暂时均不可用，未扣费。请稍后重试。");
        }
        if requires_anlas {
            return UpstreamError::new(
                503,
                "本次出图需要消耗 Anlas，本站暂无可用账号，未扣费。请改回免费参数。",
            );
        }
        if v5_free {
            return UpstreamError::local(
                403,
                "V5 暂停，未扣费。请使用 V4.5。",
                gate_core::reasons::V5_EXHAUSTED,
            );
        }
        UpstreamError::new(
            503,
            "本站 NovelAI 账号暂时均在限流或不可用，未扣费。请稍后重试。",
        )
    }

    /// 后台「上游令牌池」的状态行（不含当天计数，由调用方补）。
    pub(crate) async fn status_rows(&self) -> Vec<(Arc<Token>, TokenFields)> {
        let pool = self.state.lock().await;
        pool.tokens
            .iter()
            .map(|t| (t.clone(), t.fields()))
            .collect()
    }
}

/// 这把 Token 当日免费 V5 还没到后台设的上限（含已预留的；上限 0 = 不限）。
async fn under_v5_limit(hooks: &dyn Hooks, t: &Token) -> Result<bool, UpstreamError> {
    let (limit, pending) = t.with(|f| (f.v5_daily_limit, f.pending_v5));
    if limit == 0 {
        return Ok(true);
    }
    let used = hooks
        .upstream_v5_counter(&t.token_id, &hooks.day())
        .await
        .map_err(UpstreamError::internal)?;
    Ok(used + pending < limit)
}

/// 这把 Token 现在还能接免费 V5：最近没被确认见底，当日 V5 也没到上限。
async fn v5_room(hooks: &dyn Hooks, t: &Token, now: f64) -> Result<bool, UpstreamError> {
    Ok(now >= t.with(|f| f.v5_out_until) && under_v5_limit(hooks, t).await?)
}

fn renumber(tokens: &[Arc<Token>]) {
    for (i, t) in tokens.iter().enumerate() {
        t.with(|f| f.position = i + 1);
    }
}
