//! V5 官方免费额度缓存的 IO 层。判定逻辑在
//! `gate_core::allowance::AllowanceCore`（纯逻辑）；这里负责按账号串行、读阈值、请求订阅接口、
//! 回写恢复周期读数；额度偏低时写一行日志（后台「上游」页另有「低」标记）。
//!
//! 只有真实的出图请求会刷新缓存（订阅接口本身不消耗额度，但频繁查询像机器人）。

use gate_core::allowance::{self as core, AllowanceCore, Begin, Fetched};
use gate_db::Db;
use gate_upstream::client::{Allowance, BoxFuture, Hooks, NaiClient, Token};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub struct ServerAllowance {
    db: Db,
    core: Mutex<AllowanceCore>,
    locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// 单调时钟起点：缓存新鲜度按单调时钟算。
    epoch: std::time::Instant,
}

/// V5 余量「低」的线（缓存刷新更勤、上游页标「低」）：固定 20%。后台的「V5 提示线」（`v5_alert_threshold`）
/// 10-23 随 Medium 自动档删除，库里的旧值不再读。
pub async fn read_alert_threshold(_db: &Db) -> Result<i64, gate_db::DbError> {
    Ok(core::DEFAULT_THRESHOLD)
}

impl ServerAllowance {
    pub fn new(db: Db) -> Self {
        Self {
            db,
            core: Mutex::new(AllowanceCore::new()),
            locks: Mutex::new(HashMap::new()),
            epoch: std::time::Instant::now(),
        }
    }

    fn mono(&self) -> f64 {
        // 加一个大偏移：缺省值 -1e9 依赖单调时钟远大于 0，保证没读过时排最后
        self.epoch.elapsed().as_secs_f64() + 1e6
    }

    /// 绑定上游客户端，得到可以交给 `NaiClient::request` / `image_stream` 的 [`Allowance`]。
    pub fn with<'a, H: Hooks>(&'a self, nai: &'a NaiClient<H>) -> Bound<'a, H> {
        Bound {
            cache: self,
            nai,
            paid_cost: 0.0,
        }
    }

    /// 后台「V5 额度」面板（`snapshot(pool)`）。
    pub async fn snapshot(&self, token_ids: &[String]) -> Result<Value, gate_db::DbError> {
        let threshold = read_alert_threshold(&self.db).await?;
        let now_wall = crate::time::now();
        let now_mono = self.mono();
        let c = self.core.lock().expect("allowance");
        Ok(c.snapshot(
            token_ids.iter().map(String::as_str),
            threshold,
            now_wall,
            now_mono,
        ))
    }

    /// 这些上游账号最近一次读到的 Anlas 余额之和（读不到为 None）。
    pub fn anlas_total(&self, token_ids: &[String]) -> Option<i64> {
        self.core
            .lock()
            .expect("allowance")
            .anlas_total(token_ids.iter().map(String::as_str))
    }

    /// 这个上游账号最近一次读到的 Anlas 余额（读不到为 None）。
    pub fn anlas_of(&self, token_id: &str) -> Option<i64> {
        self.core
            .lock()
            .expect("allowance")
            .rows
            .get(token_id)
            .and_then(|r| r.anlas)
    }

    async fn resolve_with<H: Hooks>(
        &self,
        nai: &NaiClient<H>,
        token: &Token,
    ) -> Result<bool, String> {
        let lock = self
            .locks
            .lock()
            .expect("allowance")
            .entry(token.token_id.clone())
            .or_default()
            .clone();
        let _g = lock.lock().await;
        let unavailable = || core::UNAVAILABLE_MSG.to_string();
        let threshold = read_alert_threshold(&self.db)
            .await
            .map_err(|_| unavailable())?;
        match self.core.lock().expect("allowance").check_cache(
            &token.token_id,
            self.mono(),
            threshold,
        ) {
            Begin::Unavailable => return Err(unavailable()),
            Begin::Cached => return Ok(false),
            Begin::Fetch => {}
        }
        // 与这把 Token 生图时同一个浏览器指纹
        let url = format!(
            "{}/user/subscription",
            nai.image_host().trim_end_matches('/')
        );
        let fetched = tokio::time::timeout(
            Duration::from_secs(8),
            nai.get_json_with(
                &token.token,
                token.profile_index(),
                &url,
                Duration::from_secs(8),
                core::MAX_BODY,
                false,
            ),
        )
        .await;
        let (status, retry_after, body) = match &fetched {
            Ok(Ok((status, headers, body))) => (
                Some(*status),
                headers
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("")
                    .to_string(),
                Some(body.clone()),
            ),
            // 响应体超过 65536 字节时拿不到状态码：按「没拿到响应」处理，退避同样是 60 秒
            _ => (None, String::new(), None),
        };
        if status == Some(401) {
            nai.isolate_unauthorized(token).await;
        }
        let f = match status {
            None => Fetched::Failed,
            Some(s) => Fetched::Response {
                status: s,
                retry_after: &retry_after,
                body: if s == 200 { body.as_deref() } else { None },
            },
        };
        let resolved = self.core.lock().expect("allowance").finish_resolve(
            &token.token_id,
            &f,
            crate::time::now(),
            self.mono(),
            threshold,
        );
        if let Some(seconds) = resolved.note_period {
            self.note_period(&token.token_id, seconds).await;
        }
        if resolved.low {
            tracing::warn!(account = %token.token_id, "V5 low allowance");
        }
        resolved.result.map_err(|()| unavailable())
    }

    /// `_note_period`：第一次用时从库里读样本，写失败只忽略。
    async fn note_period(&self, token_id: &str, seconds: i64) {
        let loaded = self.core.lock().expect("allowance").periods_loaded();
        if !loaded {
            let raw = self.db.get_setting(core::PERIOD_KEY).await.ok().flatten();
            self.core
                .lock()
                .expect("allowance")
                .load_periods(raw.as_deref());
        }
        let text =
            self.core
                .lock()
                .expect("allowance")
                .note_period(token_id, seconds, crate::time::now());
        if let Ok(text) = text {
            let _ = self.db.set_setting(core::PERIOD_KEY, &text).await;
        }
    }
}

/// 绑定了上游客户端的额度查询。
pub struct Bound<'a, H: Hooks> {
    cache: &'a ServerAllowance,
    nai: &'a NaiClient<H>,
    /// 这次请求如果按 Anlas 付的估价（选号 / V5 确认时和账号余额比）。
    paid_cost: f64,
}

impl<H: Hooks> Bound<'_, H> {
    /// 带上这次请求按 Anlas 付的估价：余额不够的账号不接要花 Anlas 的图。
    pub fn paid_cost(mut self, anlas: f64) -> Self {
        self.paid_cost = anlas;
        self
    }
}

impl<H: Hooks> Allowance for Bound<'_, H> {
    fn resolve<'a>(&'a self, token: &'a Token) -> BoxFuture<'a, Result<bool, String>> {
        Box::pin(self.cache.resolve_with(self.nai, token))
    }

    fn anlas_balance(&self, token_id: &str) -> Option<i64> {
        self.cache.anlas_of(token_id)
    }

    fn paid_cost(&self) -> f64 {
        self.paid_cost
    }
}
