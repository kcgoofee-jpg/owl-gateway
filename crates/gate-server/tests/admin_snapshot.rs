//! 后台「成员 / 记录」接口的路由级快照测试。
//!
//! `tests/snapshots/admin/<场景>.jsonl` 的每一行是一步（第一行是场景说明，最后一行是收尾）：同一份合成种子
//! （`tests/snapshots/admin/seed_*.sql`）、同一个冻结时间，按顺序发同样的请求，记录每一步的状态码、响应头里的
//! Content-Type / 下载文件名、响应内容，以及标了 `snap` 的步骤之后各表**有变化的行**。
//! 响应原文逐字节一致（含 JSON 键顺序、浮点写法）：本身就是紧凑 JSON 的响应存成 JSON 值，其余存原文。
//!
//! 时间是进程级冻结的，所以这个文件里只放这一个测试（独占一个测试进程）。
//! 归一化：新建 Key 的随机串统一成 `nai-<rand>`；缩略图字节（编码器不同）只记是不是 JPEG。
//!
//! 更新：`UPDATE_SNAPSHOT=1 cargo test -p gate-server --test admin_snapshot`，然后审阅 `git diff`。

mod common;
use axum::http::HeaderMap;
use common::{Admin, Payload, Resp, Style, send_with, setup};
use gate_db::Db;
use gate_server::AppState;
use gate_snapshot::snapshot::Suite;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;

const M: &str = env!("CARGO_MANIFEST_DIR");

fn seed_sql(file: &str) -> String {
    std::fs::read_to_string(Path::new(M).join("tests/snapshots/admin").join(file)).expect("读种子")
}

/// 新建 Key 的随机串统一成 nai-<rand>（`nai-` + 32 位 urlsafe 字符，后面不再连着 urlsafe 字符）。
fn norm_text(s: &str) -> String {
    let is_url = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-';
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i..].starts_with(&['n', 'a', 'i', '-']) {
            let run = chars[i + 4..].iter().take_while(|c| is_url(**c)).count();
            if run == 32 {
                out.push_str("nai-<rand>");
                i += 4 + run;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

// ------------------------------------------------------------------ 重放
async fn send(state: &AppState, step: &Value, cookie: &str) -> Resp {
    let method = step["method"].as_str().unwrap();
    let path = step["path"].as_str().unwrap();
    let anon = step["anon"].as_bool().unwrap_or(false);
    // 步骤里的头覆盖默认头
    let hdrs: Vec<(&str, &str)> = step["headers"]
        .as_object()
        .map(|h| {
            h.iter()
                .map(|(k, v)| (k.as_str(), v.as_str().unwrap()))
                .collect()
        })
        .unwrap_or_default();
    let body = if !step["json"].is_null() {
        Payload::Json(step["json"].clone())
    } else if let Some(raw) = step["raw"].as_str() {
        Payload::Raw(raw.to_string())
    } else {
        Payload::None
    };
    send_with(
        Style::BROWSER,
        state,
        method,
        path,
        (!anon).then_some(cookie),
        body,
        &hdrs,
    )
    .await
}

// ------------------------------------------------------------------ 库内容：按行记录变化

const SNAP_TABLES: [&str; 11] = [
    "api_keys",
    "key_tags",
    "discord_registrations",
    "discord_bans",
    "coupons",
    "admin_actions",
    "share_state",
    "site_settings",
    "daily_quota_offsets",
    "counters",
    "key_donations",
];

/// 每张表：rowid → 行（行里第一列是 `_rowid`）。随机串已归一化。
type Tables = BTreeMap<&'static str, BTreeMap<i64, Value>>;

async fn tables(db: &Db) -> Tables {
    let mut out = Tables::new();
    for t in SNAP_TABLES {
        let rows = common::query(
            db,
            &format!("SELECT rowid AS _rowid, * FROM {t} ORDER BY rowid"),
        )
        .await;
        let rows = rows
            .into_iter()
            .map(|r| {
                let row: Value = serde_json::from_str(&norm_text(&r.to_string())).unwrap();
                (row["_rowid"].as_i64().unwrap(), row)
            })
            .collect();
        out.insert(t, rows);
    }
    out
}

fn row_counts(t: &Tables) -> Value {
    Value::Object(
        t.iter()
            .map(|(k, v)| (k.to_string(), json!(v.len())))
            .collect(),
    )
}

/// 两次之间有变化的行：`{表: {"set": [新增或改过的整行], "del": [被删的 _rowid]}}`。
fn tables_diff(before: &Tables, after: &Tables) -> Map<String, Value> {
    let mut out = Map::new();
    for t in SNAP_TABLES {
        let (b, a) = (&before[t], &after[t]);
        let set: Vec<Value> = a
            .iter()
            .filter(|(id, row)| b.get(id) != Some(row))
            .map(|(_, row)| row.clone())
            .collect();
        let del: Vec<i64> = b.keys().filter(|id| !a.contains_key(id)).copied().collect();
        if set.is_empty() && del.is_empty() {
            continue;
        }
        let mut m = Map::new();
        if !set.is_empty() {
            m.insert("set".into(), Value::Array(set));
        }
        if !del.is_empty() {
            m.insert("del".into(), json!(del));
        }
        out.insert(t.to_string(), Value::Object(m));
    }
    out
}

// ------------------------------------------------------------------ 响应内容

fn zip_view(blob: &[u8]) -> Value {
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(blob)).expect("是 zip");
    let mut files = Map::new();
    for i in 0..z.len() {
        let mut f = z.by_index(i).unwrap();
        let name = f.name().map(|n| n.to_string()).unwrap_or_default();
        let mut data = Vec::new();
        f.read_to_end(&mut data).unwrap();
        let v = if name.ends_with(".csv") || name.ends_with(".txt") {
            Value::String(norm_text(&String::from_utf8(data).unwrap()))
        } else {
            json!({"sha256": hex::encode(Sha256::digest(&data)), "len": data.len()})
        };
        files.insert(name, v);
    }
    Value::Object(files)
}

/// 响应内容的记录方式（键名, 值）：
/// - zip：每个条目一项（csv / txt 存文本，其余存哈希和长度）；
/// - 图片：JPEG 只记 `{"jpeg": true}`（编码器不同，字节不比），别的格式存哈希和长度；
/// - 本身就是紧凑 JSON 的存 `json`（读 diff 方便），否则存原文 `text`。
fn body_view(headers: &HeaderMap, body: &[u8]) -> (&'static str, Value) {
    if body.starts_with(b"PK\x03\x04") {
        return ("zip", zip_view(body));
    }
    let ctype = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if ctype.starts_with("image/") {
        let v = if body.starts_with(&[0xff, 0xd8]) && body.len() >= 40 {
            json!({"jpeg": true})
        } else {
            json!({"sha256": hex::encode(Sha256::digest(body)), "len": body.len()})
        };
        return ("image", v);
    }
    let text = norm_text(&String::from_utf8_lossy(body));
    if let Ok(v) = serde_json::from_str::<Value>(&text) {
        let compact = serde_json::to_string(&v).unwrap();
        if compact == text {
            return ("json", v);
        }
    }
    ("text", Value::String(text))
}

/// 回放一个场景（一份快照文件），返回其中的请求步数。
async fn run_variant(variant: &str) -> usize {
    let mut suite = Suite::open(M, &format!("admin/{variant}"));
    let cases = suite.cases();
    let meta = &cases[0].raw;
    assert_eq!(meta["kind"], "meta", "{variant}：第一行必须是场景说明");
    assert_eq!(meta["variant"], variant);
    gate_server::time::freeze(Some(meta["now"].as_f64().unwrap()));
    let env = setup(&[]).await;
    let state = env.state.clone();
    let sql = seed_sql(meta["seed"].as_str().unwrap());
    state
        .db
        .write(move |tx| tx.execute_batch(&sql))
        .await
        .expect("灌种子");
    // 场景要求的整体平移（early = main 的种子整体早 4 小时，按天计数的行早一天）
    if let Some(shift) = meta["shift_sql"].as_str() {
        let shift = shift.to_string();
        state
            .db
            .write(move |tx| tx.execute_batch(&shift))
            .await
            .expect("平移种子");
    }
    // 登录（会留下一条「登录后台」的操作记录）
    let cookie = Admin::login(&state).await.cookie;
    let status = 200;

    let mut seen = tables(&state.db).await;
    suite.record(0, Some(json!({"login": status, "rows": row_counts(&seen)})));
    let mut steps = 0;
    for (i, case) in cases.iter().enumerate().skip(1) {
        let step = &case.raw;
        if step["kind"] == "final" {
            let now = tables(&state.db).await;
            let mut out = Map::new();
            out.insert("rows".into(), row_counts(&now));
            let diff = tables_diff(&seen, &now);
            if !diff.is_empty() {
                out.insert("db".into(), Value::Object(diff));
            }
            suite.record(i, Some(Value::Object(out)));
            continue;
        }
        steps += 1;
        if let Some(sql) = step["sql"].as_str() {
            let sql = sql.to_string();
            state
                .db
                .write(move |tx| tx.execute_batch(&sql))
                .await
                .expect("步骤前的 SQL");
        }
        if let Some(at) = step["at"].as_f64() {
            gate_server::time::freeze(Some(at));
        }
        let Resp {
            status,
            headers,
            body,
        } = send(&state, step, &cookie).await;

        let mut out = Map::new();
        out.insert("status".into(), json!(status));
        for (key, header) in [
            ("ctype", "content-type"),
            ("disposition", "content-disposition"),
        ] {
            if let Some(v) = headers.get(header).and_then(|v| v.to_str().ok()) {
                out.insert(key.into(), json!(v));
            }
        }
        let (key, view) = body_view(&headers, &body);
        out.insert(key.into(), view);
        if step["snap"] == json!(true) {
            let now = tables(&state.db).await;
            out.insert("db".into(), Value::Object(tables_diff(&seen, &now)));
            seen = now;
        }
        suite.record(i, Some(Value::Object(out)));
    }
    suite.finish();
    steps
}

#[tokio::test]
async fn admin_routes_match_snapshot() {
    let main = run_variant("main").await;
    let merge = run_variant("merge").await;
    let early = run_variant("early").await;
    gate_server::time::freeze(None);
    // 快照没被意外截短
    assert!(
        main >= 170 && merge == 1 && early == 2,
        "{main} {merge} {early}"
    );
}
