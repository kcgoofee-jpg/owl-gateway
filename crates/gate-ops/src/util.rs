//! 各子命令共用的小工具：时间、路径、环境文件。

use jiff::{Timestamp, tz::TimeZone};
use std::path::{Path, PathBuf};

/// `time.time()`。
pub fn now_secs() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// 本机时区（Python 的 `time.localtime()` 看进程 TZ；探测失败按 UTC）。
pub fn local_tz() -> TimeZone {
    TimeZone::try_system().unwrap_or(TimeZone::UTC)
}

/// `time.strftime(fmt, time.localtime(ts))`；`tz` 由调用方传入，测试里固定成 Asia/Shanghai。
pub fn fmt_ts(ts: f64, tz: &TimeZone, fmt: &str) -> String {
    let secs = ts.floor() as i64;
    Timestamp::from_second(secs)
        .unwrap_or(Timestamp::UNIX_EPOCH)
        .to_zoned(tz.clone())
        .strftime(fmt)
        .to_string()
}

/// 近似 `du -h`：1024 进制，向上取整，小于 10 的保留一位小数（「3.2M」「120K」）。
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "K", "M", "G", "T"];
    if bytes < 1024 {
        return format!("{bytes}B");
    }
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if v < 10.0 {
        format!("{:.1}{}", (v * 10.0).ceil() / 10.0, UNITS[i])
    } else {
        format!("{:.0}{}", v.ceil(), UNITS[i])
    }
}

/// 把文件路径放进 SQLite 的 `file:` URI：只需处理 `%` `?` `#`（其余字符 SQLite 原样接受）。
pub fn sqlite_uri(path: &Path, query: &str) -> String {
    let raw = path.to_string_lossy();
    let mut enc = String::with_capacity(raw.len() + 8);
    for c in raw.chars() {
        match c {
            '%' => enc.push_str("%25"),
            '?' => enc.push_str("%3f"),
            '#' => enc.push_str("%23"),
            _ => enc.push(c),
        }
    }
    if query.is_empty() {
        format!("file:{enc}")
    } else {
        format!("file:{enc}?{query}")
    }
}

/// 数据库路径：`--db` > `NAI_GATE_DB` > `$DATA_DIR/nai_gate.db` > `data/nai_gate.db`。
pub fn resolve_db(flag: Option<&Path>) -> PathBuf {
    if let Some(p) = flag {
        return p.to_path_buf();
    }
    if let Some(p) = env_nonempty("NAI_GATE_DB") {
        return PathBuf::from(p);
    }
    data_dir().join("nai_gate.db")
}

/// `$DATA_DIR`（与应用一致，默认 `data`）。
pub fn data_dir() -> PathBuf {
    PathBuf::from(env_nonempty("DATA_DIR").unwrap_or_else(|| "data".into()))
}

pub fn env_nonempty(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

/// `.env` 文件路径：`NAI_GATE_ENV`，默认 `/opt/service-tools/.env`。
pub fn env_file() -> PathBuf {
    env_nonempty("NAI_GATE_ENV")
        .unwrap_or_else(|| "/opt/service-tools/.env".to_string())
        .into()
}

/// 配置项：进程环境变量优先，其次 `path` 指向的 `.env` 文件；都没有返回空串。
pub fn env_or_file(path: &Path, key: &str) -> String {
    env_nonempty(key).unwrap_or_else(|| env_file_value(path, key))
}

/// 升序数组的分位数（最近秩：不小于 q 比例的样本都 ≤ 它），q ∈ (0,1]；空输入返回 None。
pub fn nearest_rank<T: Copy>(sorted: &[T], q: f64) -> Option<T> {
    let n = sorted.len();
    if n == 0 {
        return None;
    }
    let idx = ((q * n as f64).ceil() as usize).clamp(1, n) - 1;
    Some(sorted[idx])
}

/// 读取 `.env` 风格文件里的某个键（与旧版 patrol.py 的 `env_value` 一致，见标签 py-final：取第一处命中，
/// 去掉首尾空白与成对引号字符；文件读不到按空串）。
pub fn env_file_value(path: &Path, key: &str) -> String {
    let Ok(bytes) = std::fs::read(path) else {
        return String::new();
    };
    let text = String::from_utf8_lossy(&bytes);
    let prefix = format!("{key}=");
    for line in text.lines() {
        let line = gate_core::pyc::strip(line);
        if let Some(rest) = line.strip_prefix(&prefix) {
            let v = gate_core::pyc::strip(rest);
            // Python 的 str.strip('"') 会去掉两端所有连续的引号字符
            return v.trim_matches('"').trim_matches('\'').to_string();
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_rank_quantiles() {
        let v: Vec<i64> = (1..=10).collect();
        assert_eq!(nearest_rank(&v, 0.9), Some(9));
        assert_eq!(nearest_rank(&v, 1.0), Some(10));
        assert_eq!(nearest_rank::<i64>(&[], 0.9), None);
        assert_eq!(nearest_rank(&[7], 0.9), Some(7));
        let f: Vec<f64> = (1..=1000).map(f64::from).collect();
        assert_eq!(nearest_rank(&f, 0.999), Some(999.0));
        assert_eq!(nearest_rank(&f[..10], 0.999), Some(10.0));
    }

    #[test]
    fn human_size_like_du() {
        assert_eq!(human_size(512), "512B");
        assert_eq!(human_size(3 * 1024 * 1024 + 100), "3.1M");
        assert_eq!(human_size(120 * 1024), "120K");
    }

    #[test]
    fn env_file_parsing() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join(".env");
        std::fs::write(&p, "  A=\"x y\"\nB='q'\nC=\nA=second\nD=a=b\n").unwrap();
        assert_eq!(env_file_value(&p, "A"), "x y");
        assert_eq!(env_file_value(&p, "B"), "q");
        assert_eq!(env_file_value(&p, "C"), "");
        assert_eq!(env_file_value(&p, "D"), "a=b");
        assert_eq!(env_file_value(&p, "NOPE"), "");
    }

    #[test]
    fn env_or_file_prefers_process_env() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join(".env");
        std::fs::write(&p, "PATH=from-file\nOWL_TEST_ONLY_IN_FILE=v\n").unwrap();
        assert_eq!(env_or_file(&p, "OWL_TEST_ONLY_IN_FILE"), "v");
        assert_eq!(env_or_file(&p, "OWL_TEST_NOWHERE"), "");
        // PATH 一定在进程环境里：环境变量优先于文件
        assert_ne!(env_or_file(&p, "PATH"), "from-file");
    }

    #[test]
    fn sqlite_uri_escapes_specials() {
        assert_eq!(
            sqlite_uri(Path::new("/a b/c?d#e%f.db"), "mode=ro"),
            "file:/a b/c%3fd%23e%25f.db?mode=ro"
        );
    }
}
