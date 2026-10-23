//! 后台管理的上游 Token 存储（`data/upstream_tokens.json`，权限 600）。与旧版 `app/token_store.py` 同格式。
//!
//! 原始 Token 不写进 SQLite（数据库备份里不会有它）。文件存在时它是令牌池的唯一来源；
//! 不存在、损坏或为空时回退到 `.env` 的 `NAI_TOKENS`。

use serde_json::{Value, json};
use std::io::Write;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub token: String,
    pub allow_anlas: bool,
}

/// `^pst-[A-Za-z0-9_\-]{16,200}$`
pub fn valid_token(token: &str) -> bool {
    let Some(rest) = token.strip_prefix("pst-") else {
        return false;
    };
    (16..=200).contains(&rest.len())
        && rest
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// 读取；文件不存在 / 损坏 / 没有合法 Token 时返回 None。
pub fn load(path: &Path) -> Option<Vec<Entry>> {
    let text = std::fs::read_to_string(path).ok()?;
    let data: Value = serde_json::from_str(&text).ok()?;
    let tokens = data.get("tokens")?.as_array()?;
    let entries: Vec<Entry> = tokens
        .iter()
        .filter_map(|e| {
            let token = e.get("token")?.as_str()?.to_string();
            if !valid_token(&token) {
                return None;
            }
            let allow_anlas = e.get("allow_anlas").and_then(Value::as_bool) == Some(true);
            Some(Entry { token, allow_anlas })
        })
        .collect();
    (!entries.is_empty()).then_some(entries)
}

/// 原子写入：先写同目录 `.tmp`（强制 0600，覆盖残留旧文件的权限）→ fsync → rename。
pub fn save(path: &Path, entries: &[Entry]) -> std::io::Result<()> {
    let tmp = path.with_file_name(format!(
        "{}.tmp",
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("upstream_tokens.json")
    ));
    let body = json!({
        "tokens": entries.iter().map(|e| json!({"token": e.token, "allow_anlas": e.allow_anlas})).collect::<Vec<_>>()
    });
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        f.write_all(serde_json::to_string(&body)?.as_bytes())?;
        f.flush()?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_validation() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("upstream_tokens.json");
        assert!(load(&p).is_none());
        let good = "pst-abcdefghijklmnop_-12";
        save(
            &p,
            &[Entry {
                token: good.into(),
                allow_anlas: true,
            }],
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            load(&p).unwrap(),
            vec![Entry {
                token: good.into(),
                allow_anlas: true
            }]
        );
        assert!(!valid_token("pst-short"));
        assert!(!valid_token("abc-abcdefghijklmnopq"));
        std::fs::write(&p, r#"{"tokens": [{"token": "pst-bad"}]}"#).unwrap();
        assert!(load(&p).is_none());
    }
}
