//! `audit`（生成记录：提示词整理、披露文案、设置解析、图片收录和缩略图）的快照测试。
//!
//! - `audit/prompts`：提示词整理，输出（或异常的类型 + 消息）逐字节一致；
//! - `audit/settings`：库里的原始设置串 → 记录开关 / 保留时长 / 披露文案（真实 SQLite 库）；
//! - `audit/images`：图片只记「决定」——收不收、类型串、缩略图尺寸与四个采样点的颜色（编码器的字节不存）。
//!
//! 更新：`UPDATE_SNAPSHOT=1 cargo test -p gate-server --test audit_snapshot`，然后审阅 `git diff`。
//! 测试函数名都以 `audit_` 开头，`cargo test -p gate-server audit` 能选中。

use base64::Engine;
use gate_core::pyext::PyErr;
use gate_db::Db;
use gate_server::audit;
use gate_snapshot::Case;
use gate_snapshot::snapshot::{Suite, check};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::{Cursor, Write};

const M: &str = env!("CARGO_MANIFEST_DIR");

/// 每个函数（或每类输入）在快照里至少要留的用例数：抽样、删用例时不会悄悄丢掉一个函数。
const MIN_PER_FN: usize = 15;

fn ok(v: Value) -> Value {
    json!({ "ret": v })
}

fn py_err(e: PyErr) -> Value {
    json!({"err": {"type": e.type_name(), "msg": e.message()}})
}

/// 数一数每个函数有多少条用例、其中多少条是异常。
#[derive(Default)]
struct Tally {
    cases: BTreeMap<String, usize>,
    errors: usize,
}

impl Tally {
    fn add(&mut self, name: &str, out: &Value) {
        *self.cases.entry(name.to_string()).or_default() += 1;
        if out.get("err").is_some() {
            self.errors += 1;
        }
    }

    #[track_caller]
    fn assert_each_at_least(&self, names: &[&str], k: usize, min_errors: usize) {
        for n in names {
            let got = self.cases.get(*n).copied().unwrap_or(0);
            assert!(got >= k, "{n} 在快照里只有 {got} 条（至少 {k} 条）");
        }
        assert!(
            self.errors >= min_errors,
            "快照里应有至少 {min_errors} 条异常用例，实际 {}",
            self.errors
        );
    }
}

// ------------------------------------------------------------------ 提示词

#[test]
fn audit_prompts_snapshot() {
    let mut tally = Tally::default();
    check(M, "audit/prompts", |c| {
        let fn_name = c.get("fn").and_then(Value::as_str).unwrap();
        let arg = c.arg(0);
        let out = match fn_name {
            "capture_prompts" => audit::capture_prompts(arg)
                .map(|(p, n, e)| ok(json!([p, n, e])))
                .unwrap_or_else(py_err),
            "prompt_texts" => audit::prompt_texts(arg)
                .map(|(p, n)| ok(json!([p, n])))
                .unwrap_or_else(py_err),
            "_captions" => audit::captions(Some(arg))
                .map(|(base, chars)| ok(json!([base, chars])))
                .unwrap_or_else(py_err),
            other => panic!("未知函数 {other}"),
        };
        tally.add(fn_name, &out);
        Some(out)
    });
    // 快照里确实有抛异常的用例（不是全绿的假象）
    tally.assert_each_at_least(
        &["capture_prompts", "prompt_texts", "_captions"],
        MIN_PER_FN,
        20,
    );
}

// ------------------------------------------------------------------ 设置读取（真实 SQLite）

#[tokio::test]
async fn audit_settings_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path().join("gate.db")).unwrap();
    let mut suite = Suite::open(M, "audit/settings");
    let cases = suite.cases();
    let mut tally = Tally::default();
    for (i, c) in cases.iter().enumerate() {
        let fn_name = c.get("fn").and_then(Value::as_str).unwrap();
        // 把这一例的原始设置串写进库（先清空：缺失的键必须真的没有这一行）
        db.write(|tx| tx.execute_batch("DELETE FROM site_settings"))
            .await
            .unwrap();
        for (k, v) in c.raw["kv"].as_object().unwrap() {
            db.set_setting(k, v.as_str().unwrap()).await.unwrap();
        }
        let got = match fn_name {
            "audit_flags" => audit::audit_flags(&db)
                .await
                .map(|(p, t, d)| json!([p, t, d])),
            "audit_image_keep_hours" => audit::audit_image_keep_hours(&db).await.map(|v| json!(v)),
            "audit_image_cap_mb" => audit::audit_image_cap_mb(&db).await.map(|v| json!(v)),
            other => panic!("未知函数 {other}"),
        };
        let out = match got {
            Ok(v) => ok(v),
            Err(e) => json!({"err": e.to_string()}),
        };
        tally.add(fn_name, &out);
        suite.record(i, Some(out));
    }
    suite.finish();
    tally.assert_each_at_least(
        &["audit_image_keep_hours", "audit_image_cap_mb"],
        MIN_PER_FN,
        0,
    );
    // 开关只有这么几种不同的原始串
    tally.assert_each_at_least(&["audit_flags"], 10, 0);
}

// ------------------------------------------------------------------ 图片

fn b64(s: &str) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD
        .decode(s)
        .expect("快照里的 base64")
}

/// recipe → 字节：blob = head + 零填充；zip = 每个条目一个 blob。
fn blob(spec: &Value) -> Vec<u8> {
    let mut data = spec
        .get("head")
        .and_then(Value::as_str)
        .map(b64)
        .unwrap_or_default();
    data.resize(spec["len"].as_u64().unwrap() as usize, 0);
    data
}

fn build(recipe: &Value) -> Vec<u8> {
    match recipe["kind"].as_str().unwrap() {
        "blob" => blob(recipe),
        "zip" => {
            let method = if recipe["method"] == "deflate" {
                zip::CompressionMethod::Deflated
            } else {
                zip::CompressionMethod::Stored
            };
            let mut buf = Cursor::new(Vec::new());
            let mut z = zip::ZipWriter::new(&mut buf);
            for e in recipe["entries"].as_array().unwrap() {
                z.start_file(
                    e["name"].as_str().unwrap(),
                    zip::write::SimpleFileOptions::default().compression_method(method),
                )
                .unwrap();
                z.write_all(&blob(e)).unwrap();
            }
            z.finish().unwrap();
            buf.into_inner()
        }
        other => panic!("未知 recipe {other}"),
    }
}

fn payload_of(c: &Case) -> Vec<u8> {
    let p = &c.raw["payload"];
    match p.get("$b").and_then(Value::as_str) {
        Some(s) => b64(s),
        None => build(&p["recipe"]),
    }
}

/// 缩略图只记尺寸和四个象限中心的颜色；字节不存（编码器版本不同字节就不同）。
fn thumb_view(name: &str, jpeg: Option<Vec<u8>>) -> Value {
    let Some(jpeg) = jpeg else {
        return Value::Null;
    };
    assert!(jpeg.starts_with(b"\xff\xd8\xff"), "{name}：缩略图不是 JPEG");
    let img = image::load_from_memory_with_format(&jpeg, image::ImageFormat::Jpeg)
        .unwrap_or_else(|e| panic!("{name}：缩略图解不开：{e}"))
        .to_rgb8();
    let (w, h) = img.dimensions();
    let points = [(0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)];
    let samples: Vec<Value> = points
        .iter()
        .map(|(fx, fy)| {
            let px = img.get_pixel((f64::from(w) * fx) as u32, (f64::from(h) * fy) as u32);
            json!([px[0], px[1], px[2]])
        })
        .collect();
    json!({"size": [w, h], "samples": samples})
}

/// 存下来的原图只记类型、长度和哈希。
fn full_view(data: Option<Vec<u8>>, ctype: String) -> Value {
    let mut m = Map::new();
    m.insert("type".into(), json!(ctype));
    match data {
        Some(d) => {
            m.insert("len".into(), json!(d.len()));
            m.insert("sha256".into(), json!(hex_of(&Sha256::digest(&d))));
        }
        None => {
            m.insert("stored".into(), json!(false));
        }
    }
    Value::Object(m)
}

#[test]
fn audit_images_snapshot() {
    let (mut accepted, mut rejected) = (0, 0);
    check(M, "audit/images", |c| {
        let name = c.raw["name"].as_str().unwrap().to_string();
        let payload = payload_of(c);
        let thumb = audit::make_thumbnail(&payload);
        if thumb.is_some() {
            accepted += 1;
        } else {
            rejected += 1;
        }
        let (data, ctype) = audit::full_image(&payload);
        Some(json!({
            "thumb": thumb_view(&name, thumb),
            "full": full_view(data, ctype),
        }))
    });
    assert!(
        accepted > 50 && rejected > 30,
        "accepted={accepted} rejected={rejected}"
    );
}

fn hex_of(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
