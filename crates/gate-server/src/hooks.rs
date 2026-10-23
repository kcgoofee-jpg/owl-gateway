//! 网关给上游客户端注入的能力：服务日、告警、账号保护、上游计数、结果校验。

use crate::alerts::Alerter;
use crate::guard::Guard;
use bytes::Bytes;
use gate_core::clock::Clock;
use gate_db::Db;
use gate_upstream::client::{BoxFuture, HookResult, Hooks};
use gate_upstream::pool::{Cooldown, decode_cooldowns, encode_cooldowns};
use std::sync::Arc;

/// 各上游账号的冷却状态（审查 2026-10-17 M-4），格式见 `gate_upstream::pool::encode_cooldowns`。
pub const COOLDOWNS_KEY: &str = "upstream_token_cooldowns";

pub struct GateHooks {
    db: Db,
    clock: Clock,
    guard: Arc<Guard>,
    alerter: Arc<Alerter>,
    /// 冷却存库串行化：读出 → 合并 → 写回 必须一次做完，不然两次 403 挨得近时后一次会被覆盖。
    cooldown_save: Arc<tokio::sync::Mutex<()>>,
}

impl GateHooks {
    pub fn new(db: Db, clock: Clock, guard: Arc<Guard>, alerter: Arc<Alerter>) -> Self {
        Self {
            db,
            clock,
            guard,
            alerter,
            cooldown_save: Arc::default(),
        }
    }
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
        self.clock.day(crate::time::now())
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
