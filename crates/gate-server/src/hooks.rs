//! 网关给上游客户端注入的能力：服务日、告警、账号保护、上游计数、结果校验。

use crate::alerts::Alerter;
use crate::guard::Guard;
use bytes::Bytes;
use gate_core::clock::Clock;
use gate_db::Db;
use gate_upstream::client::{BoxFuture, HookResult, Hooks};
use gate_upstream::pool::{Cooldown, decode_cooldowns, encode_cooldowns};
use std::collections::HashSet;
use std::sync::Arc;

/// 各上游账号的冷却状态（审查 2026-10-17 M-4），格式见 `gate_upstream::pool::encode_cooldowns`。
pub const COOLDOWNS_KEY: &str = "upstream_token_cooldowns";
/// 明确收到上游 401 的凭据指纹；不随重启清除，不存储原始凭据。
pub const INVALID_TOKENS_KEY: &str = "upstream_invalid_token_fingerprints";

pub struct GateHooks {
    db: Db,
    clock: Clock,
    guard: Arc<Guard>,
    alerter: Arc<Alerter>,
    /// 冷却存库串行化：读出 → 合并 → 写回 必须一次做完，不然两次 403 挨得近时后一次会被覆盖。
    cooldown_save: Arc<tokio::sync::Mutex<()>>,
    invalid_token_save: Arc<tokio::sync::Mutex<()>>,
}

impl GateHooks {
    pub fn new(db: Db, clock: Clock, guard: Arc<Guard>, alerter: Arc<Alerter>) -> Self {
        Self {
            db,
            clock,
            guard,
            alerter,
            cooldown_save: Arc::default(),
            invalid_token_save: Arc::default(),
        }
    }
}

/// 解析隔离指纹。损坏或非预期格式按错误处理，避免静默恢复可能已失效的凭据。
pub fn decode_invalid_tokens(raw: Option<&str>) -> Result<HashSet<String>, String> {
    let Some(raw) = raw else {
        return Ok(HashSet::new());
    };
    let values: Vec<String> =
        serde_json::from_str(raw).map_err(|_| "上游凭据隔离记录格式损坏".to_string())?;
    if values.iter().any(|id| {
        id.len() != 22
            || !id.starts_with("token-")
            || !id[6..].bytes().all(|b| b.is_ascii_hexdigit())
    }) {
        return Err("上游凭据隔离记录包含无效指纹".to_string());
    }
    Ok(values.into_iter().collect())
}

async fn save_invalid_tokens(db: &Db, ids: &HashSet<String>) -> gate_db::Result<()> {
    let mut sorted: Vec<_> = ids.iter().cloned().collect();
    sorted.sort();
    db.set_setting(INVALID_TOKENS_KEY, &serde_json::to_string(&sorted).unwrap())
        .await
}

async fn remove_invalid_token(db: &Db, token_id: &str) -> Result<(), String> {
    let raw = db
        .get_setting(INVALID_TOKENS_KEY)
        .await
        .map_err(|e| e.to_string())?;
    let mut ids = decode_invalid_tokens(raw.as_deref())?;
    if ids.remove(token_id) {
        save_invalid_tokens(db, &ids)
            .await
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

async fn save_cooldown(db: &Db, token_id: &str, c: Cooldown, now: f64) -> gate_db::Result<()> {
    let mut map = decode_cooldowns(db.get_setting(COOLDOWNS_KEY).await?.as_deref());
    let merged = map.get(token_id).map_or(c, |old| old.merge(c));
    map.insert(token_id.to_string(), merged);
    map.retain(|_, c| c.live(now));
    db.set_setting(COOLDOWNS_KEY, &encode_cooldowns(&map)).await
}

fn boxed<E: std::error::Error + Send + Sync + 'static>(
    e: E,
) -> Box<dyn std::error::Error + Send + Sync> {
    Box::new(e)
}

impl Hooks for GateHooks {
    fn day(&self) -> String {
        crate::reqctx::current()
            .and_then(|ctx| ctx.settlement_day())
            .unwrap_or_else(|| self.clock.day(crate::time::now()))
    }

    fn now(&self) -> f64 {
        crate::time::now()
    }

    /// 上游客户端报的事件：Token 失效（401）和连续 403 进入冷却会私信站长；单次 402 / 403 只写日志。
    fn event(&self, kind: &str, message: &str, cooldown: f64) {
        if matches!(kind, "upstream_401" | "upstream_403_cooldown") {
            self.alerter.notify(kind, message, cooldown);
        } else {
            tracing::info!(target: "upstream", kind, "{message}");
        }
    }

    fn cooldown_changed(&self, token_id: &str, cooldown: Cooldown) {
        let Ok(rt) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let (db, lock, id) = (
            self.db.clone(),
            self.cooldown_save.clone(),
            token_id.to_string(),
        );
        rt.spawn(async move {
            let _serial = lock.lock().await;
            if let Err(e) = save_cooldown(&db, &id, cooldown, crate::time::now()).await {
                tracing::warn!(error = %e, "上游账号冷却存库失败（重启后会提前恢复试探）");
            }
        });
    }

    fn unauthorized<'a>(&'a self, token_id: &'a str) -> BoxFuture<'a, HookResult<()>> {
        Box::pin(async move {
            let _serial = self.invalid_token_save.lock().await;
            let raw = self.db.get_setting(INVALID_TOKENS_KEY).await?;
            let mut ids = decode_invalid_tokens(raw.as_deref()).map_err(std::io::Error::other)?;
            ids.insert(token_id.to_string());
            save_invalid_tokens(&self.db, &ids).await?;
            Ok(())
        })
    }

    fn clear_unauthorized<'a>(&'a self, token_id: &'a str) -> BoxFuture<'a, HookResult<()>> {
        Box::pin(async move {
            let _serial = self.invalid_token_save.lock().await;
            remove_invalid_token(&self.db, token_id)
                .await
                .map_err(std::io::Error::other)?;
            Ok(())
        })
    }

    fn has_guard(&self) -> bool {
        true
    }

    fn token_block_reason<'a>(
        &'a self,
        token_id: &'a str,
        day: &'a str,
    ) -> BoxFuture<'a, HookResult<Option<(String, String)>>> {
        Box::pin(async move {
            let r = self
                .guard
                .token_block_reason(token_id, day, crate::time::now())
                .await
                .map_err(boxed)?;
            Ok(r.map(|r| (r.text().to_string(), r.code().to_string())))
        })
    }

    fn post_jitter(&self) -> (f64, f64) {
        self.guard.post_jitter()
    }

    fn upstream_v5_counter<'a>(
        &'a self,
        token_id: &'a str,
        day: &'a str,
    ) -> BoxFuture<'a, HookResult<i64>> {
        Box::pin(async move {
            self.db
                .get_upstream_counter(token_id, day)
                .await
                .map(|c| c.v5)
                .map_err(boxed)
        })
    }

    fn bump_upstream_v5<'a>(
        &'a self,
        token_id: &'a str,
        day: &'a str,
    ) -> BoxFuture<'a, HookResult<()>> {
        Box::pin(async move {
            self.db
                .bump_upstream_v5_counter(token_id, day)
                .await
                .map_err(boxed)
        })
    }

    fn bump_upstream_images<'a>(
        &'a self,
        token_id: &'a str,
        day: &'a str,
        n: i64,
    ) -> BoxFuture<'a, HookResult<()>> {
        Box::pin(async move {
            self.db
                .bump_upstream_image_counter(token_id, day, n)
                .await
                .map_err(boxed)
        })
    }

    fn validate_image_result(
        &self,
        body: Bytes,
        expected_images: usize,
    ) -> BoxFuture<'static, bool> {
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                crate::media::validate_result(&body, "generate-image", Some(expected_images))
                    .is_ok()
            })
            .await
            .unwrap_or(false)
        })
    }
}
