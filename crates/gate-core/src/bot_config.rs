//! Discord 机器人（奶妹）的运行状态（心跳）的纯逻辑部分。
//!
//! 机器人只管成员命令；跑图分享、最近动作之类的扩展功能不存在，站点设置里残留的
//! `bot_gallery_*` / `bot_events` 行不再读写（只增不减，留在库里无害）。
//!
//! 不在这里的（服务层负责，都是 `site_settings` 表的键值读写）：
//! - 机器人每分钟上报一次心跳：服务层把 `status` 交给 [`status_to_store`]，结果写进 `bot_status`；
//! - 后台「机器人状态」卡片：服务层读 `bot_status`（缺省 `"{}"`），交给 [`snapshot`]。

use crate::values;
use serde_json::{Map, Value};

/// `site_settings` 里存心跳的键。
pub const STATUS_KEY: &str = "bot_status";
/// 心跳每分钟一次，3 分钟没收到算离线
pub const ONLINE_WINDOW: f64 = 180.0;

/// 库里的状态坏了：后台接口回 500。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BotConfigError {
    /// 库里的 JSON 坏了（不是合法 JSON）→ 500
    #[error("ValueError: {0}")]
    BadJson(String),
    /// 库里的 JSON 类型不对（要对象）
    #[error("AttributeError: {0}")]
    WrongType(String),
    /// 状态里的字段类型不对
    #[error("TypeError: {0}")]
    Type(String),
}

/// 心跳里留下的字段（白名单）；其余的键一律丢弃。
const STATUS_FIELDS: [&str; 5] = ["user", "guild", "latency_ms", "ready_at", "version"];

/// `report` 的心跳部分：只留白名单字段，整体覆盖 `bot_status`。`status` 必须是对象。
pub fn status_to_store(status: &Map<String, Value>, now: f64) -> String {
    let mut keep = Map::new();
    for k in STATUS_FIELDS {
        if let Some(v) = status.get(k) {
            keep.insert(k.to_string(), v.clone());
        }
    }
    keep.insert("seen_at".into(), values::float(now));
    crate::json::dumps(&Value::Object(keep))
}

/// 在线判断：`bool(seen and now - seen < 180)`。`seen` 是状态里的 `seen_at`（没有 = None）。
/// 为真但不是数字时报错（`Err`）。
pub fn online(seen_at: Option<&Value>, now: f64) -> Result<bool, BotConfigError> {
    let Some(seen) = seen_at else {
        return Ok(false);
    };
    if !values::enabled(seen) {
        return Ok(false);
    }
    match values::number(seen) {
        Some(s) => Ok(now - s < ONLINE_WINDOW),
        None => Err(BotConfigError::Type(
            "unsupported operand type(s) for -: 'float' and ...".into(),
        )),
    }
}

/// 后台「机器人状态」卡片的数据：`{"status": {…心跳字段, "online": bool}}`。
/// `status_raw` 是 `bot_status` 在库里的原文（没有 / 空串 = 还没收到过心跳）。
pub fn snapshot(status_raw: Option<&str>, now: f64) -> Result<Value, BotConfigError> {
    let text = match status_raw {
        Some(s) if !s.is_empty() => s,
        _ => "{}",
    };
    let parsed: Value =
        serde_json::from_str(text).map_err(|e| BotConfigError::BadJson(e.to_string()))?;
    let Value::Object(mut status) = parsed else {
        return Err(BotConfigError::WrongType(
            "object has no attribute 'get'".into(),
        ));
    };
    let is_online = online(status.get("seen_at"), now)?;
    status.insert("online".into(), Value::Bool(is_online));
    let mut out = Map::new();
    out.insert("status".into(), Value::Object(status));
    Ok(Value::Object(out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn heartbeat_keeps_only_whitelisted_fields() {
        let status = json!({
            "user": "naimei", "guild": "测试服", "latency_ms": 42, "ready_at": 1.5,
            "version": "3.0", "ai_ready": true, "ai_today": 3, "token": "x"
        });
        let stored = status_to_store(status.as_object().unwrap(), 1000.0);
        assert_eq!(
            serde_json::from_str::<Value>(&stored).unwrap(),
            json!({"user":"naimei","guild":"测试服","latency_ms":42,"ready_at":1.5,"version":"3.0","seen_at":1000.0})
        );
    }

    #[test]
    fn snapshot_reports_online_within_window() {
        let raw = status_to_store(&Map::new(), 1000.0);
        let on = snapshot(Some(&raw), 1000.0 + ONLINE_WINDOW - 1.0).unwrap();
        assert_eq!(on, json!({"status": {"seen_at": 1000.0, "online": true}}));
        let off = snapshot(Some(&raw), 1000.0 + ONLINE_WINDOW).unwrap();
        assert_eq!(off["status"]["online"], false);
        // 还没收到过心跳
        for empty in [None, Some("")] {
            assert_eq!(
                snapshot(empty, 1000.0).unwrap(),
                json!({"status": {"online": false}})
            );
        }
    }

    #[test]
    fn snapshot_rejects_broken_status() {
        assert!(matches!(
            snapshot(Some("not json"), 0.0),
            Err(BotConfigError::BadJson(_))
        ));
        assert!(matches!(
            snapshot(Some("[1]"), 0.0),
            Err(BotConfigError::WrongType(_))
        ));
        assert!(matches!(
            snapshot(Some(r#"{"seen_at": "x"}"#), 0.0),
            Err(BotConfigError::Type(_))
        ));
    }
}
