//! 后台管理的上游 Token 存储（`data/upstream_tokens.json`，权限 600）。与旧版 `app/token_store.py` 同格式。
//!
//! 原始 Token 不写进 SQLite（数据库备份里不会有它）。文件存在时它是令牌池的唯一来源；
//! 不存在或不含有效的旧格式 Token 时回退到 `.env` 的 `NAI_TOKENS`。
//!
//! 官方号是 `pst-…`，没有 `image_host`。拼车号是 `NAI-…`，必须带自己的 HTTPS 地址
//! （测试允许 `http://127.0.0.1` / `http://localhost`）。

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::io::Write;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub token: String,
    #[serde(default)]
    pub allow_anlas: bool,
    /// 拼车网关的 API 根地址。官方号为 `None`（走全局 `NAI_IMAGE_HOST`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_host: Option<String>,
}

impl Entry {
    pub fn official(token: impl Into<String>, allow_anlas: bool) -> Self {
        Self {
            token: token.into(),
            allow_anlas,
            image_host: None,
        }
    }

    pub fn share(
        token: impl Into<String>,
        image_host: impl Into<String>,
        allow_anlas: bool,
    ) -> Self {
        Self {
            token: token.into(),
            allow_anlas,
            image_host: Some(image_host.into()),
        }
    }

    pub fn is_share(&self) -> bool {
        self.image_host.is_some()
    }

    /// 去掉空白、规范化 Host；不合法则 `None`。
    pub fn normalized(self) -> Option<Self> {
        let token = self.token.trim().to_string();
        let image_host = match self.image_host {
            None => None,
            Some(h) => Some(normalize_share_host(&h)?),
        };
        let entry = Self {
            token,
            allow_anlas: self.allow_anlas,
            image_host,
        };
        entry.valid().then_some(entry)
    }

    pub fn valid(&self) -> bool {
        match self.image_host.as_deref() {
            None => valid_official_token(&self.token),
            Some(host) => {
                valid_share_token(&self.token)
                    && normalize_share_host(host).as_deref() == Some(host)
            }
        }
    }
}

fn rest_ok(rest: &str) -> bool {
    (16..=200).contains(&rest.len())
        && rest
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// `^pst-[A-Za-z0-9_\-]{16,200}$`
pub fn valid_official_token(token: &str) -> bool {
    token.strip_prefix("pst-").is_some_and(rest_ok)
}

/// `^NAI-[A-Za-z0-9_\-]{16,200}$`
pub fn valid_share_token(token: &str) -> bool {
    token.strip_prefix("NAI-").is_some_and(rest_ok)
}

/// 官方号（无 Host 的 `pst-`）。拼车号请用 [`valid_share_token`]。
pub fn valid_token(token: &str) -> bool {
    valid_official_token(token)
}

/// 拼车网关地址：https 公网，或测试用 `http://127.0.0.1[:port]` / `http://localhost[:port]`。
/// 不要路径、查询、片段或用户信息。
pub fn normalize_share_host(raw: &str) -> Option<String> {
    let s = raw.trim();
    let (scheme, rest) = if let Some(r) = s.strip_prefix("https://") {
        ("https", r)
    } else {
        ("http", s.strip_prefix("http://")?)
    };
    let rest = rest.trim_end_matches('/');
    if rest.is_empty()
        || rest.contains('/')
        || rest.contains('?')
        || rest.contains('#')
        || rest.contains('@')
        || rest.contains('\\')
        || rest.contains(' ')
    {
        return None;
    }
    let (host, port) = match rest.rsplit_once(':') {
        Some((h, p)) if !p.is_empty() && p.len() <= 5 && p.bytes().all(|b| b.is_ascii_digit()) => {
            let n: u32 = p.parse().ok()?;
            if !(1..=65535).contains(&n) {
                return None;
            }
            (h, Some(p))
        }
        _ => (rest, None),
    };
    if host.is_empty() {
        return None;
    }
    let local = host.eq_ignore_ascii_case("localhost") || host == "127.0.0.1";
    match scheme {
        "http" if local => {}
        "https"
            if host
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-') => {}
        _ => return None,
    }
    Some(match port {
        Some(p) => format!("{scheme}://{host}:{p}"),
        None => format!("{scheme}://{host}"),
    })
}

fn parse_entries(value: &Value) -> Result<Vec<Entry>, std::io::Error> {
    let tokens = value.as_array().ok_or_else(invalid_file)?;
    Ok(tokens
        .iter()
        .filter_map(|e| {
            serde_json::from_value::<Entry>(e.clone())
                .ok()?
                .normalized()
        })
        .collect())
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
        let entries = parse_entries(&candidate["tokens"])?;
        return Ok((!entries.is_empty()).then_some(entries));
    }
    let entries = parse_entries(&file["tokens"])?;
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
        let old = vec![Entry::official("pst-synthetic-old-0001", false)];
        let new = vec![Entry::official("pst-synthetic-new-0002", false)];
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
        save(&p, &[Entry::official(good, true)]).unwrap();
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            load(&p, None).unwrap().unwrap(),
            vec![Entry::official(good, true)]
        );
        assert!(!valid_token("pst-short"));
        assert!(!valid_token("abc-abcdefghijklmnopq"));
        std::fs::write(&p, r#"{"tokens": [{"token": "pst-bad"}]}"#).unwrap();
        assert!(load(&p, None).unwrap().is_none());
    }

    #[test]
    fn share_entries_roundtrip_and_drop_invalid() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("upstream_tokens.json");
        let official = Entry::official("pst-synthetic-official-0001", false);
        let share = Entry::share("NAI-synthetic-share-0001", "https://share.example", true);
        save(&p, &[official.clone(), share.clone()]).unwrap();
        assert_eq!(load(&p, None).unwrap().unwrap(), vec![official, share]);

        std::fs::write(
            &p,
            r#"{"tokens":[
                {"token":"pst-synthetic-official-0001"},
                {"token":"NAI-synthetic-share-0001"},
                {"token":"NAI-synthetic-share-0001","image_host":"https://share.example/path"},
                {"token":"pst-synthetic-official-0001","image_host":"https://share.example"},
                {"token":"NAI-synthetic-share-0001","image_host":"https://share.example"}
            ]}"#,
        )
        .unwrap();
        let loaded = load(&p, None).unwrap().unwrap();
        assert_eq!(loaded.len(), 2);
        assert!(loaded[0].image_host.is_none());
        assert_eq!(
            loaded[1].image_host.as_deref(),
            Some("https://share.example")
        );
        assert!(normalize_share_host("http://127.0.0.1:18081").is_some());
        assert!(normalize_share_host("http://example.com").is_none());
    }
}
