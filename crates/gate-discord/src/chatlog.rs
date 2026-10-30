//! 对话留档：每轮一行 JSONL，落盘到挂载卷（训练语料 / 纪念册两用）。
//!
//! 只在配置了 `NAIMEI_CHAT_LOG` 时启用；写入失败只打 warn，绝不影响对话本身。
//! 内容包含成员消息，文件留在站长服务器上，不外发、不进仓库。
//! 字段里的 `status`：`ok`（完整回答）/ `quota`（上游额度不足，兜底文案）/ `failed`（其他失败），
//! 筛选语料时按 `status == "ok"` 取。

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::json;
use tracing::warn;

#[derive(Clone)]
pub struct ChatLog {
    path: PathBuf,
}

impl ChatLog {
    pub fn new(path: &str) -> Self {
        ChatLog {
            path: PathBuf::from(path),
        }
    }

    /// 记一轮对话。`user_text` / `assistant` 都是最终落库文本（assistant 已过出口过滤）。
    #[allow(clippy::too_many_arguments)]
    pub fn record(
        &self,
        guild_id: u64,
        channel_id: u64,
        user_id: &str,
        user_name: &str,
        is_owner: bool,
        user_text: &str,
        assistant: &str,
        model: &str,
        latency_ms: u128,
        status: &str,
    ) {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let line = json!({
            "ts": ts,
            "guild_id": guild_id,
            "channel_id": channel_id,
            "user_id": user_id,
            "user_name": user_name,
            "is_owner": is_owner,
            "user": user_text,
            "assistant": assistant,
            "model": model,
            "latency_ms": latency_ms,
            "status": status,
        });
        if let Some(dir) = self.path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        match OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            Ok(mut f) => {
                if let Err(e) = writeln!(f, "{line}") {
                    warn!("[chatlog] 写入失败：{e}");
                }
            }
            Err(e) => warn!("[chatlog] 打开日志文件失败：{e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_appends_parseable_jsonl() {
        let dir = std::env::temp_dir().join(format!("naimei-chatlog-test-{}", std::process::id()));
        let path = dir.join("chat.jsonl");
        let log = ChatLog::new(path.to_str().unwrap());
        log.record(
            1,
            2,
            "u1",
            "alice",
            true,
            "你好",
            "哼，才不想理你呢～",
            "glm-5.3-flash",
            1234,
            "ok",
        );
        log.record(
            1,
            2,
            "u2",
            "bob",
            false,
            "领key",
            "",
            "glm-5.3-flash",
            0,
            "quota",
        );
        let content = std::fs::read_to_string(&path).unwrap();
        let mut lines = content.lines();
        let first: serde_json::Value = serde_json::from_str(lines.next().unwrap()).unwrap();
        assert_eq!(first["user_name"], "alice");
        assert_eq!(first["is_owner"], true);
        assert_eq!(first["status"], "ok");
        assert_eq!(first["latency_ms"], 1234);
        let second: serde_json::Value = serde_json::from_str(lines.next().unwrap()).unwrap();
        assert_eq!(second["status"], "quota");
        assert_eq!(second["assistant"], "");
        assert!(lines.next().is_none());
        std::fs::remove_dir_all(&dir).ok();
    }
}
