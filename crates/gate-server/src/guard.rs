//! 账号保护的运行层：`gate_core::guard::GuardCore`（纯逻辑）+ 读写库 + 告警。
//! 对应旧版 `app/guard.py` 里带 `await db` 的方法（load / save / token_block_reason）。
//! 每小时上限的 429 减半 / 每日上调（on_upstream_429 / adapt_daily）已删除。核心状态放在同步 Mutex 里，从不跨 await 持锁：先查库，再算。

use gate_core::guard::GuardCore;
use gate_core::pyext::PyErr;
use gate_core::reasons::Reason;
use gate_db::Db;
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::sync::Mutex;

pub struct Guard {
    core: Mutex<GuardCore>,
    db: Db,
    /// 有出图条目被移除（出完 / 被拒 / 取消）：叫醒同一把 Key 排在后面的那张（见 `admission::image_admission`）。
    pub released: tokio::sync::Notify,
}

#[derive(Debug, thiserror::Error)]
pub enum GuardIoError {
    #[error(transparent)]
    Db(#[from] gate_db::DbError),
    #[error("{0:?}")]
    Guard(PyErr),
}

impl From<PyErr> for GuardIoError {
    fn from(e: PyErr) -> Self {
        GuardIoError::Guard(e)
    }
}

impl Guard {
    pub fn new(db: Db, tz: &str, reset_hour: i64) -> Self {
        Self {
            core: Mutex::new(GuardCore::new(tz, reset_hour)),
            db,
            released: tokio::sync::Notify::new(),
        }
    }

    /// 移除一个出图条目，并叫醒等着轮到自己的同 Key 请求。
    pub fn release_image(&self, key_id: i64, entry_id: Option<i64>) {
        self.with(|g| g.release_image(key_id, entry_id));
        self.released.notify_waiters();
    }

    /// 只读 / 同步操作直接拿核心（不得跨 await）。
    pub fn with<R>(&self, f: impl FnOnce(&mut GuardCore) -> R) -> R {
        f(&mut self.core.lock().expect("guard"))
    }

    /// 启动时从 site_settings 读 `guard_<field>`。
    pub async fn load(&self) -> Result<(), GuardIoError> {
        let mut stored = BTreeMap::new();
        for f in gate_core::guard::FIELDS.iter() {
            let key = gate_core::guard::setting_key(f.name);
            if let Some(v) = self.db.get_setting(&key).await? {
                stored.insert(key, v);
            }
        }
        self.with(|c| c.load_settings(&stored))?;
        let caps = self
            .db
            .get_setting(gate_core::guard::TOKEN_DAILY_CAPS_KEY)
            .await?;
        self.with(|c| {
            c.token_daily_caps = gate_core::guard::parse_token_daily_caps(caps.as_deref());
        });
        Ok(())
    }

    /// 后台保存：校验 → 写库 → 改内存。
    pub async fn save(
        &self,
        body: &Map<String, Value>,
    ) -> Result<Vec<(String, i64)>, GuardIoError> {
        let updates = GuardCore::validate_save(body)?;
        if !updates.is_empty() {
            let writes: Vec<(String, String)> = updates
                .iter()
                .map(|(k, v)| (gate_core::guard::setting_key(k), v.to_string()))
                .collect();
            self.db.set_settings_bulk(&writes).await?;
            self.with(|c| c.apply_updates(&updates));
        }
        Ok(updates)
    }

    /// 这把上游 Token 现在能不能出图（每日上限 / 熔断）。
    pub async fn token_block_reason(
        &self,
        token_id: &str,
        day: &str,
        now: f64,
    ) -> Result<Option<Reason>, GuardIoError> {
        let c = self.db.get_upstream_counter(token_id, day).await?;
        Ok(self.with(|g| g.token_block_reason(token_id, c.images as f64, now)))
    }

    /// 每张出完后歇的 (最少, 最多) 秒数（后台「额度与保护」可调，出图节奏唯一的旋钮）。
    pub fn post_jitter(&self) -> (f64, f64) {
        let (lo, hi) = self.with(|g| g.post_jitter());
        (lo as f64, hi as f64)
    }
}
