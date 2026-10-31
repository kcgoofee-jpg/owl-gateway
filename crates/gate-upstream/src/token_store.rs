//! 后台管理的上游 Token 存储（`data/upstream_tokens.json`，权限 600）。与旧版 `app/token_store.py` 同格式。
//!
//! 原始 Token 不写进 SQLite（数据库备份里不会有它）。文件存在时它是令牌池的唯一来源；
//! 不存在或不含有效的旧格式 Token 时回退到 `.env` 的 `NAI_TOKENS`。

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::io::Write;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub token: String,
    #[serde(default)]
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

/// The SQLite revision commits the matching candidate. Before that commit the
/// previous credentials remain authoritative, including after a process crash.
pub fn load(path: &Path, revision: Option<&str>) -> std::io::Result<Option<Vec<Entry>>> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let file: Value = serde_json::from_slice(&bytes).map_err(|_| invalid_file())?;
    if let Some(candidate) = file.get("candidate")
        && candidate["revision"]
            .as_str()
            .is_some_and(|id| Some(id) == revision)
    {
        return serde_json::from_value(candidate["tokens"].clone())
            .map(Some)
            .map_err(|_| invalid_file());
    }
    let tokens = file["tokens"].as_array().ok_or_else(invalid_file)?;
    let entries: Vec<Entry> = tokens
        .iter()
        .filter_map(|e| {
            let token = e.get("token")?.as_str()?;
            valid_token(token).then(|| Entry {
                token: token.to_owned(),
                allow_anlas: e["allow_anlas"].as_bool().unwrap_or(false),
            })
        })
        .collect();
    Ok((!entries.is_empty()).then_some(entries))
}

fn invalid_file() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, "上游凭据文件格式无效")
}

pub fn stage(
    path: &Path,
    previous: &[Entry],
    next: &[Entry],
    revision: &str,
) -> std::io::Result<()> {
    write(
        path,
        &json!({"tokens": previous, "candidate": {"revision":revision, "tokens":next}}),
    )
}

pub fn save(path: &Path, entries: &[Entry]) -> std::io::Result<()> {
    write(path, &json!({"tokens": entries}))
}

fn write(path: &Path, body: &Value) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    tmp.write_all(&serde_json::to_vec(body)?)?;
    tmp.as_file().sync_all()?;
    tmp.persist(path)?;
    std::fs::File::open(dir)?.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restart_uses_the_revision_committed_in_sqlite() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("upstream_tokens.json");
        let make = |token: &str| Entry {
            token: token.to_owned(),
            allow_anlas: false,
        };
        let old = vec![make("pst-synthetic-old-0001")];
        let new = vec![make("pst-synthetic-new-0002")];
        stage(&file, &old, &new, "rev2").unwrap();
        assert_eq!(load(&file, None).unwrap(), Some(old.clone()));
        assert_eq!(load(&file, Some("rev1")).unwrap(), Some(old));
        assert_eq!(load(&file, Some("rev2")).unwrap(), Some(new));
    }

    #[test]
    fn roundtrip_and_validation() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("upstream_tokens.json");
        assert!(load(&p, None).unwrap().is_none());
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
            load(&p, None).unwrap().unwrap(),
            vec![Entry {
                token: good.into(),
                allow_anlas: true
            }]
        );
        assert!(!valid_token("pst-short"));
        assert!(!valid_token("abc-abcdefghijklmnopq"));
        std::fs::write(&p, r#"{"tokens": [{"token": "pst-bad"}]}"#).unwrap();
        assert!(load(&p, None).unwrap().is_none());
    }
}
