//! 合成演示数据（公开仓库：全部是假的）。
//!
//! 它建了 8 把带名字的 Key、各自的 Discord 领取记录和 400 条用量日志；这里在此之上补了计数器、
//! 生成记录、捐赠、券、黑名单、状态页事件、操作日志和状态码统计，让后台每一页都有东西可看。
//!
//! 约定（Playwright 端到端依赖，别随手改）：
//! - 第 0 号成员「月见草」（Discord `900000000000000000`）是 `cookies.json` 里的成员会话；它**没有**问卷记录、
//!   没有券（首页 / 问卷页的用例按「新成员」来测），也不会是后台成员列表（按今日用量排序）的第一位；
//! - 后台成员列表的第一位是「Kestrel」——后台写操作用例（重新生成 Key、停用 / 启用……）拿它开刀，
//!   不会弄坏上面那位成员的会话。所以 Kestrel 的今日出图数必须是最多的；
//! - 生成记录里只有缩略图、没有原图：后台「清空服务器上的全部原图」在没有原图时是禁用的，用例断言了这一点；
//!   今天每一张成功出图都配一条生成记录（记账恒等式：生成记录条数 = 成功出图张数，测试里断言）；
//! - 捐赠只有「已处理」的：有待处理的，后台 Discord 侧栏角标会变成「! 1」，深色下白字配红底对比度只有 2.76，
//!   端到端的无障碍用例（axe）会判 serious（前端样式问题，不是种子的问题）；
//! - 所有 Discord ID 都在 `9000000000000000xx` 段，Token 都是 `nai-demo…` / `pst-demo…`。

use anyhow::{Context, Result};
use gate_core::clock::Clock;
use gate_core::reasons;
use gate_db::Db;
use gate_db::repo::audit::NewAudit;
use gate_db::repo::discord::BanInfo;
use gate_db::repo::keys::NewKey;
use gate_db::repo::registration::NewRegistration;
use gate_db::repo::usage::{NewLog, Success};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

pub const ADMIN_PASSWORD: &str = "dev-admin-password-123";
/// 会话签名密钥（写进 `<数据目录>/secret_key`）。
pub const SECRET: &str = "0000000000000000dev-secret-key-not-for-production-use-0000";

const DAY: f64 = 86400.0;

/// 成员名字（顺序即 Key 编号：1 号起）。
const NAMES: [&str; 8] = [
    "月见草",
    "白鸟",
    "nightjar",
    "雾岛",
    "tester-04",
    "柚子",
    "Kestrel",
    "长尾山雀",
];
/// 今日出图张数（与 NAMES 同序）。Kestrel 最多，见模块说明。
const TODAY_OK: [usize; 8] = [9, 7, 6, 5, 4, 3, 24, 2];

const MODELS: [&str; 3] = [
    "nai-diffusion-4-5-full",
    "nai-diffusion-4-5-curated",
    "nai-diffusion-5-full",
];
const PROMPTS: [(&str, &str); 4] = [
    (
        "1girl, solo, silver hair, night sky, stars, looking at viewer",
        "lowres, bad anatomy",
    ),
    (
        "2girls, tea party, garden, soft light, watercolor",
        "lowres, bad hands",
    ),
    (
        "cat ears, hoodie, rainy street, neon lights, from side",
        "lowres, blurry",
    ),
    (
        "scenery, mountain lake, sunrise, mist, no humans",
        "lowres, text",
    ),
];

pub struct SeededKey {
    pub id: i64,
    pub name: &'static str,
    pub token: String,
    pub discord_id: String,
}

pub struct Seeded {
    /// 按 NAMES 的顺序。
    pub keys: Vec<SeededKey>,
}

pub fn discord_id(i: usize) -> String {
    (900_000_000_000_000_000u64 + i as u64).to_string()
}

pub fn token(i: usize) -> String {
    format!("nai-demo{i:02}{}", "x".repeat(30))
}

/// 一条要写的请求记录。
struct Event {
    ts: f64,
    key: usize,
    kind: &'static str,
    outcome: Outcome,
}

#[derive(Clone, Copy, PartialEq)]
enum Outcome {
    Ok,
    Rejected,
    Error,
}

/// 本地网关用的假上游 Token（演示数据里的用量日志按它的 token_id 记账）。
pub const DEV_UPSTREAM_TOKEN: &str = "pst-devdevdevdevdevdevdev";

pub async fn seed(db: &Db, clock: &Clock, now: f64) -> Result<Seeded> {
    let mut rng = StdRng::seed_from_u64(7);
    let keys = seed_members(db, now).await?;
    seed_usage(db, clock, now, &keys, &mut rng).await?;
    seed_member_extras(db, now, &keys).await?;
    seed_records(db, now).await?;
    seed_site(db, now).await?;
    seed_reconciliation(db, now).await?;
    Ok(Seeded { keys })
}

// ------------------------------------------------------------------ 成员

async fn seed_members(db: &Db, now: f64) -> Result<Vec<SeededKey>> {
    let mut keys = Vec::new();
    for (i, name) in NAMES.iter().enumerate() {
        let created = now - DAY * (i as f64 + 2.0);
        let (token, did) = (token(i), discord_id(i));
        let key: NewKey = serde_json::from_value(serde_json::json!({
            "name": name, "token": token, "daily_images": 104, "daily_anlas": 50.0, "daily_v5": 10,
            "monthly_anlas": 500.0, "daily_text_tokens": 150000, "rpm": 5,
            "expires_at": now + DAY * 20.0, "features": "image,vibe,tags", "discord_id": did,
        }))?;
        let row = db.create_key(&key, created).await?;
        let id = row["id"].as_i64().context("新建的 Key 没有 id")?;
        db.save_registration(&NewRegistration {
            discord_id: &did,
            key_id: id,
            now: created,
            username: Some(&format!("user{i}")),
            display_name: Some(name),
            avatar: None,
            key_name: name,
            member_role: false,
        })
        .await?;
        keys.push(SeededKey {
            id,
            name,
            token,
            discord_id: did,
        });
    }
    Ok(keys)
}

// ------------------------------------------------------------------ 用量日志与计数器

fn pick<'a, T>(rng: &mut StdRng, items: &'a [T]) -> &'a T {
    &items[rng.random_range(0..items.len())]
}

fn rid(rng: &mut StdRng) -> String {
    format!("{:016x}", rng.random::<u64>())
}

fn events(now: f64, clock: &Clock, rng: &mut StdRng) -> Vec<Event> {
    let today0 = clock.day_start(now);
    // 最近 7 个服务日，今天从 today0 到「一分钟前」
    let mut out = Vec::new();
    for back in 0..7u32 {
        let (lo, hi) = if back == 0 {
            (today0, (now - 60.0).max(today0 + 1.0))
        } else {
            let end = today0 - DAY * f64::from(back - 1);
            (end - DAY, end)
        };
        for (key, &today_ok) in TODAY_OK.iter().enumerate() {
            let ok = if back == 0 {
                today_ok
            } else {
                rng.random_range(4..=22)
            };
            for _ in 0..ok {
                let kind = match rng.random_range(0..100) {
                    0..=69 => "image",
                    70..=89 => "image_stream",
                    _ => "tags",
                };
                out.push(Event {
                    ts: rng.random_range(lo..hi),
                    key,
                    kind,
                    outcome: Outcome::Ok,
                });
            }
            // 另有约 14% 被拒、3% 出错
            for _ in 0..(ok * 14 / 100).max(usize::from(rng.random_bool(0.5))) {
                out.push(Event {
                    ts: rng.random_range(lo..hi),
                    key,
                    kind: "image",
                    outcome: Outcome::Rejected,
                });
            }
            if rng.random_bool(0.35) {
                out.push(Event {
                    ts: rng.random_range(lo..hi),
                    key,
                    kind: "image",
                    outcome: Outcome::Error,
                });
            }
        }
    }
    // 每把 Key 最近一次使用都在刚才（后台「最近使用」、闲置判定、停机检测都不会有怪结果）
    for key in 0..NAMES.len() {
        out.push(Event {
            ts: now - 45.0 - 240.0 * key as f64,
            key,
            kind: "image",
            outcome: Outcome::Ok,
        });
    }
    out.sort_by(|a, b| a.ts.total_cmp(&b.ts));
    out
}

/// 用量日志与计数器；顺带写生成记录——和线上一样，今天每一张成功出图都有一条生成记录
/// （记账恒等式，见文件末尾的测试），所以生成记录跟着成功出图一起写，只有缩略图、没有原图。
async fn seed_usage(
    db: &Db,
    clock: &Clock,
    now: f64,
    keys: &[SeededKey],
    rng: &mut StdRng,
) -> Result<()> {
    let audit_from = clock
        .local_midnight(&clock.day(now))
        .context("算不出今天的本地 0 点")?;
    let mut audited = 0usize;
    let rejects = [
        (reasons::KEY_RATE, "请求过于频繁，请放慢一点再试"),
        (reasons::QUEUE_FULL, "现在排队的人太多了，请稍后再试"),
        (
            reasons::MEMBER_QUOTA,
            "今日 V4.5 免费图额度已用完，每日 7 点重置",
        ),
    ];
    let errors = [
        (502, "上游图片服务暂时不可用（HTTP 502）"),
        (500, "上游图片服务内部错误（HTTP 500）"),
        (429, "上游限流（HTTP 429），已自动冷却"),
    ];
    let token_id = gate_upstream::headers::token_id(DEV_UPSTREAM_TOKEN);
    // 请求日志的来源 IP / 客户端：按 Key 固定（不动随机数序列），文档专用网段（RFC 5737）；
    // 每 4 把 Key 里有两把共用一个 IP，演示「同一个 IP 用过哪些 Key」。
    const UAS: [&str; 4] = [
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/131.0 Safari/537.36",
        "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 Version/18.0 Mobile/15E148 Safari/604.1",
        "DSH-Tavern-Image/1.2 (Windows NT 10.0)",
        "python-requests/2.32.3",
    ];
    let origin = |k: usize| {
        let host = if k % 4 == 3 { k - 1 } else { k };
        (
            format!("203.0.113.{}", 10 + host),
            UAS[k % UAS.len()].to_string(),
        )
    };
    for e in events(now, clock, rng) {
        let key = &keys[e.key];
        let (ip, client) = origin(e.key);
        let model = if e.kind == "tags" {
            ""
        } else {
            *pick(rng, &MODELS)
        };
        let common = (
            key.id,
            key.name.to_string(),
            e.kind.to_string(),
            model.to_string(),
            rid(rng),
        );
        match e.outcome {
            Outcome::Ok => {
                let image = e.kind != "tags";
                let v5 = i64::from(image && model == "nai-diffusion-5-full");
                db.record_success(
                    &Success {
                        key_id: common.0,
                        key_name: common.1,
                        kind: common.2,
                        model: common.3,
                        day: clock.day(e.ts),
                        images: i64::from(image),
                        v5,
                        legacy_free_images: i64::from(image && v5 == 0),
                        wait_ms: rng.random_range(0..20000),
                        dur_ms: rng.random_range(3000..9000),
                        up_status: 200,
                        rid: common.4,
                        token_id: Some(token_id.clone()),
                        client,
                        ip,
                        ..Default::default()
                    },
                    e.ts,
                )
                .await?;
                if image && e.ts >= audit_from {
                    let (prompt, negative) = PROMPTS[audited % PROMPTS.len()];
                    audited += 1;
                    db.add_audit(
                        &NewAudit {
                            key_id: Some(key.id),
                            key_name: key.name,
                            kind: "image",
                            model,
                            status: "ok",
                            prompt,
                            negative,
                            ..Default::default()
                        },
                        e.ts,
                        clock,
                    )
                    .await?;
                }
            }
            Outcome::Rejected => {
                let (reason, detail) = *pick(rng, &rejects);
                db.add_log(
                    &NewLog {
                        key_id: Some(common.0),
                        key_name: common.1,
                        kind: common.2,
                        model: common.3,
                        status: "rejected".into(),
                        detail: detail.into(),
                        reason: reason.into(),
                        wait_ms: rng.random_range(0..3000),
                        rid: common.4,
                        client,
                        ip,
                        ..Default::default()
                    },
                    e.ts,
                )
                .await?;
            }
            Outcome::Error => {
                let (up, detail) = *pick(rng, &errors);
                db.add_log(
                    &NewLog {
                        key_id: Some(common.0),
                        key_name: common.1,
                        kind: common.2,
                        model: common.3,
                        status: "error".into(),
                        detail: detail.into(),
                        wait_ms: rng.random_range(0..20000),
                        dur_ms: rng.random_range(3000..9000),
                        up_status: up,
                        rid: common.4,
                        token_id: Some(token_id.clone()),
                        client,
                        ip,
                        ..Default::default()
                    },
                    e.ts,
                )
                .await?;
            }
        }
    }
    Ok(())
}

/// 一份一天前的 Anlas 对账快照：「上游」页的余额推算有数可算。
async fn seed_reconciliation(db: &Db, now: f64) -> Result<()> {
    let id = gate_upstream::headers::token_id(DEV_UPSTREAM_TOKEN);
    let snapshot = serde_json::json!({
        "checked_at": now - 86400.0,
        "accounts": {id: {"fixed": 9000, "purchased": 300, "expires_at": (now + 12.0 * 86400.0) as i64}},
        "ledger": {"anlas": 0, "unconfirmed_anlas": 0, "unconfirmed_requests": 0, "last_log_id": 0},
        "balance": 9300,
        "comparison": {"reason": "first"},
    });
    db.save_reconciliation(&snapshot).await?;
    Ok(())
}

// ------------------------------------------------------------------ 成员相关：券 / 黑名单 / 捐赠

async fn seed_member_extras(db: &Db, now: f64, keys: &[SeededKey]) -> Result<()> {
    // 重置券：给白鸟、Kestrel 各一张。月见草（会话成员）不发，首页用例自己用 page.route 造券
    db.add_coupon(
        keys[1].id,
        "reset",
        5.0,
        "补偿 10-13",
        "后台",
        "v45",
        now - 3600.0,
    )
    .await?;
    db.add_coupon(
        keys[6].id,
        "reset",
        3.0,
        "唱得真好听",
        "后台",
        "v45,v5,anlas",
        now - 7200.0,
    )
    .await?;

    // 黑名单：一条系统回收、一条手动
    db.ban_discord(
        "900000000000000101",
        &BanInfo {
            reason: "连续 3 天未活动自动回收".into(),
            username: "ghost_a".into(),
            display_name: "旧成员甲".into(),
            ..Default::default()
        },
        now - 2.0 * DAY,
    )
    .await?;
    db.ban_discord(
        "900000000000000102",
        &BanInfo {
            reason: "转卖 Key（演示数据）".into(),
            username: "ghost_b".into(),
            display_name: "旧成员乙".into(),
            banned_by: "后台(203.0.*.*)".into(),
            ..Default::default()
        },
        now - 5.0 * DAY,
    )
    .await?;

    Ok(())
}

// ------------------------------------------------------------------ 操作日志 / Bug

async fn seed_records(db: &Db, now: f64) -> Result<()> {
    // 操作日志
    for (ago, actor, action, target, detail) in [
        (
            6.0 * 3600.0,
            "Discord:900000000000000003",
            "领取 Key",
            "Key #4 雾岛",
            "",
        ),
        (
            5.0 * 3600.0,
            "后台(203.0.*.*)",
            "修改 Key",
            "Key #2 白鸟",
            "每日 V5 8 → 10",
        ),
        (
            4.0 * 3600.0,
            "后台(203.0.*.*)",
            "新建 Key",
            "Key #8 长尾山雀",
            "手动创建，有效期 30 天",
        ),
        (
            3.0 * 3600.0,
            "后台(203.0.*.*)",
            "成员标签",
            "Key #3 nightjar",
            "添加标签「内测」",
        ),
        (
            2.0 * 3600.0,
            "系统",
            "动态额度",
            "",
            "每人每日 V4.5 104 → 104，V5 10 → 10",
        ),
    ] {
        db.add_admin_action(actor, action, target, detail, true, now - ago)
            .await?;
    }

    Ok(())
}

// ------------------------------------------------------------------ 站点：状态页事件 / 状态码统计

async fn seed_site(db: &Db, now: f64) -> Result<()> {
    use gate_server::routes::status::standard_body;
    // 两个已解决的事件（照 Statuspage 标准句式写）
    let first = db
        .create_incident(
            "图片生成变慢",
            "minor",
            &["upstream".to_string()],
            "investigating",
            standard_body("investigating").unwrap_or_default(),
            now - 2.0 * DAY,
        )
        .await?;
    db.add_incident_update(
        first,
        "resolved",
        standard_body("resolved").unwrap_or_default(),
        now - 2.0 * DAY + 5400.0,
    )
    .await?;
    let second = db
        .create_incident(
            "出图接口短暂不可用",
            "major",
            &["upstream".to_string(), "api".to_string()],
            "investigating",
            standard_body("investigating").unwrap_or_default(),
            now - 5.0 * DAY,
        )
        .await?;
    db.add_incident_update(
        second,
        "monitoring",
        standard_body("monitoring").unwrap_or_default(),
        now - 5.0 * DAY + 1800.0,
    )
    .await?;
    db.add_incident_update(
        second,
        "resolved",
        standard_body("resolved").unwrap_or_default(),
        now - 5.0 * DAY + 4200.0,
    )
    .await?;

    // 成员接口各 HTTP 状态码的每小时次数（后台总览与公开状态页的分布图）：最近 72 小时
    let this_hour = (now / 3600.0).floor() as i64 * 3600;
    let mut items = Vec::new();
    for h in 1..=72i64 {
        let hour = this_hour - h * 3600;
        let busy = 20 + (h * 7 % 40);
        items.push(((hour, 200u16), busy));
        if h % 3 == 0 {
            items.push(((hour, 429u16), 1 + h % 4));
        }
        if h % 11 == 0 {
            items.push(((hour, 500u16), 1));
        }
        if h % 5 == 0 {
            items.push(((hour, 401u16), 1));
        }
    }
    db.flush_status_hourly(&items).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 种子数据守住端到端依赖的约定（见模块说明）。换三个时刻各种一遍：刚过重置点、中午、重置点前一刻。
    #[tokio::test]
    async fn seed_keeps_the_conventions_e2e_relies_on() {
        let clock = Clock::new("Asia/Shanghai", 7);
        let today0 = clock.day_start(1_792_000_000.0);
        for now in [today0 + 30.0, today0 + 12.0 * 3600.0, today0 + DAY - 30.0] {
            let dir = tempfile::tempdir().unwrap();
            let db = Db::open(dir.path().join("nai_gate.db")).unwrap();
            let seeded = seed(&db, &clock, now).await.unwrap();
            let day = clock.day(now);

            // 后台成员列表按今日出图排序：第一位必须是 Kestrel，会话成员（第 0 号）不能是
            let mut today = Vec::new();
            for k in &seeded.keys {
                let c = db.get_counter(k.id, &day).await.unwrap();
                today.push((c.images, k.name));
            }
            today.sort_by(|a, b| b.cmp(a));
            assert_eq!(today[0].1, "Kestrel", "{today:?}");
            assert!(today[0].0 > today[1].0 + 5, "领先要足够多：{today:?}");

            // 记账恒等式：今天的生成记录条数 = 成功出图张数；而且只有缩略图、没有原图
            let start = clock.local_midnight(&day).unwrap();
            let count = |sql: &'static str| {
                db.read(move |c| c.query_row(sql, [start], |r| r.get::<usize, i64>(0)))
            };
            let logged = count(
                "SELECT COALESCE(SUM(images),0) FROM usage_log WHERE ts>=? AND status='ok' AND kind LIKE 'image%'",
            )
            .await
            .unwrap();
            assert!(logged > 0);
            let audited =
                count("SELECT COUNT(*) FROM generation_audit WHERE ts>=? AND status='ok'")
                    .await
                    .unwrap();
            assert_eq!(audited, logged);
            assert!(!db.path().parent().unwrap().join("audit_images").exists());

            // 会话成员没有券
            let member = seeded.keys[0].id;
            assert!(
                db.list_coupons(member, false, now)
                    .await
                    .unwrap()
                    .is_empty()
            );

            // 最近一条请求就在刚才（网关启动时的停机检测不会往操作日志里写「闲置计时重置」）
            let last = db.last_log_ts().await.unwrap().unwrap();
            assert!(now - last < 3600.0, "最近一条日志太旧：{}", now - last);
        }
    }
}
