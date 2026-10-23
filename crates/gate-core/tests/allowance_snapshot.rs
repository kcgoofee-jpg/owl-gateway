//! allowance.rs 的快照。
//!
//! 每条是一条操作轨迹：`AllowanceCore` 在固定时钟 / 假 HTTP 结果下执行的 resolve / note_period / period /
//! snapshot / 改阈值。这里用 check_cache → finish_resolve → note_period 重放，逐步记下返回值、缓存行、写库内容和
//! 「额度偏低」的判定，最后记 periods 整体（含账号插入顺序）。

mod common;

use base64::{Engine, engine::general_purpose::STANDARD};
use common::Tally;
use gate_core::allowance::{
    self, AllowanceCore, Begin, Fetched, MAX_BODY, PERIOD_KEY, SETTING, UNAVAILABLE_MSG,
};
use gate_snapshot::snapshot::check;
use gate_snapshot::{Case, f};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

const M: &str = env!("CARGO_MANIFEST_DIR");

#[test]
fn threshold_snapshot() {
    let mut n = 0;
    check(M, "allowance/threshold", |c| {
        n += 1;
        let v = if c.get("present")?.as_bool()? {
            c.arg(0).clone()
        } else {
            Value::Null
        };
        Some(json!(allowance::normalize_alert_threshold(&v)))
    });
    assert!(n >= 20, "用例太少：{n}");
}

/// 假 HTTP 的结果（拥有数据，好借给 `Fetched`）。
enum Plan {
    Failed,
    Response {
        status: u16,
        retry_after: String,
        body: Option<Vec<u8>>,
    },
}

fn plan_of(p: &Value) -> Plan {
    match p["kind"].as_str().unwrap() {
        "error" | "timeout" => Plan::Failed,
        kind => {
            let bytes: Vec<u8> = if let Some(n) = p.get("body_size").and_then(Value::as_u64) {
                vec![b'x'; n as usize]
            } else if let Some(b) = p.get("body_b64").and_then(Value::as_str) {
                STANDARD.decode(b).unwrap()
            } else {
                p["body"].as_str().unwrap().as_bytes().to_vec()
            };
            Plan::Response {
                status: p["status"].as_u64().unwrap() as u16,
                retry_after: p["retry_after"].as_str().unwrap_or("").to_string(),
                // 响应体最多读 MAX_BODY 字节，超过或读取中途出错都是 None
                body: (kind != "read_error" && bytes.len() <= MAX_BODY).then_some(bytes),
            }
        }
    }
}

fn row_json(core: &AllowanceCore, token: &str) -> Value {
    let mut m = Map::new();
    if let Some(r) = core.rows.get(token) {
        if let Some(v) = r.percent {
            m.insert("percent".into(), json!(v));
        }
        if let Some(v) = r.is_negative {
            m.insert("is_negative".into(), json!(v));
        }
        if let Some(v) = r.checked_at {
            m.insert("checked_at".into(), f(v));
        }
        if let Some(v) = r.at {
            m.insert("at".into(), f(v));
        }
        if let Some(v) = &r.error {
            m.insert("error".into(), json!(v));
        }
        if r.retry_at != 0.0 {
            m.insert("retry_at".into(), f(r.retry_at));
        }
        if let Some(v) = r.next_percent_seconds {
            m.insert("next_percent_seconds".into(), json!(v));
        }
    }
    Value::Object(m)
}

/// 写进库里的一条：键、字符数、SHA-256；不到 700 字符的连原文一起记。
fn write_json(key: &str, text: &str) -> Value {
    json!({
        "key": key,
        "len": text.chars().count(),
        "sha256": hex::encode(Sha256::digest(text.as_bytes())),
        "text": if text.chars().count() < 700 { json!(text) } else { Value::Null },
    })
}

fn unavailable() -> Value {
    json!({"err": "AllowanceUnavailable", "msg": UNAVAILABLE_MSG})
}

/// 重放一条轨迹，返回每一步的结果和最终的 periods。
fn replay(c: &Case, tally: &mut Tally) -> Value {
    let mut core = AllowanceCore::new();
    let mut kv: BTreeMap<String, String> = c.raw["init"]["kv"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string()))
        .collect();
    let threshold_of = |raw: &Value| {
        allowance::normalize_alert_threshold(&match raw {
            Value::String(_) => raw.clone(),
            _ => Value::Null,
        })
    };
    let mut results = Vec::new();
    for step in c.raw["steps"].as_array().unwrap() {
        let op = step["op"].as_str().unwrap();
        tally.add(op);
        let result = match op {
            "set_threshold" => {
                match step["raw"].as_str() {
                    Some(s) => {
                        kv.insert(SETTING.into(), s.into());
                    }
                    None => {
                        kv.remove(SETTING);
                    }
                }
                Value::Null
            }
            "resolve" => {
                let token = step["token"].as_str().unwrap();
                let mono = step["mono"].as_f64().unwrap();
                // 阈值原文就是库里当时的值
                let threshold = threshold_of(&step["threshold_raw"]);
                let (mono_after, wall_after) = (
                    step["mono_after"].as_f64().unwrap(),
                    step["wall_after"].as_f64().unwrap(),
                );
                let (mut calls, mut low) = (0, false);
                let mut writes = Vec::new();
                let out = match core.check_cache(token, mono, threshold) {
                    Begin::Unavailable => unavailable(),
                    Begin::Cached => json!({"ok": false}),
                    Begin::Fetch => {
                        calls = 1;
                        let plan = plan_of(&step["plan"]);
                        let fetched = match &plan {
                            Plan::Failed => Fetched::Failed,
                            Plan::Response {
                                status,
                                retry_after,
                                body,
                            } => Fetched::Response {
                                status: *status,
                                retry_after,
                                body: body.as_deref(),
                            },
                        };
                        let r =
                            core.finish_resolve(token, &fetched, wall_after, mono_after, threshold);
                        low = r.low;
                        if let Some(secs) = r.note_period {
                            core.load_periods(kv.get(PERIOD_KEY).map(String::as_str));
                            let text = core.note_period(token, secs, wall_after).unwrap();
                            writes.push(write_json(PERIOD_KEY, &text));
                            kv.insert(PERIOD_KEY.into(), text);
                        }
                        match r.result {
                            Ok(neg) => json!({"ok": neg}),
                            Err(()) => unavailable(),
                        }
                    }
                };
                tally.add_if(calls > 0, "fetch");
                tally.add_if(low, "low");
                json!({
                    "result": out,
                    "http_calls": calls,
                    "low": low,
                    "row": row_json(&core, token),
                    "writes": writes,
                })
            }
            "note_period" => {
                core.load_periods(kv.get(PERIOD_KEY).map(String::as_str));
                let text = core
                    .note_period(
                        step["token"].as_str().unwrap(),
                        step["seconds"].as_i64().unwrap(),
                        step["wall"].as_f64().unwrap(),
                    )
                    .unwrap();
                let writes = json!([write_json(PERIOD_KEY, &text)]);
                kv.insert(PERIOD_KEY.into(), text);
                json!({"writes": writes})
            }
            "period" => {
                let got = core.period(
                    step["token"].as_str().unwrap(),
                    step["wall"].as_f64().unwrap(),
                );
                got.map_or(Value::Null, |v| json!(v))
            }
            "snapshot" => {
                let tokens: Vec<&str> = step["tokens"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|t| t.as_str().unwrap())
                    .collect();
                core.snapshot(
                    tokens,
                    threshold_of(&step["threshold_raw"]),
                    step["wall"].as_f64().unwrap(),
                    step["mono"].as_f64().unwrap(),
                )
            }
            other => panic!("未知操作 {other}"),
        };
        results.push(result);
    }
    // 终态：periods 整体（含账号插入顺序）
    let periods = core
        .periods
        .as_ref()
        .map_or(Value::Null, |p| serde_json::from_str(&p.to_json()).unwrap());
    json!({"steps": results, "final_periods": periods})
}

fn replay_file(name: &str, tally: &mut Tally) {
    check(M, name, |c| {
        tally.add(name);
        Some(replay(c, tally))
    });
}

/// 响应体各种写法（JSON 形状、编码、超长）。
#[test]
fn bodies_snapshot() {
    let mut tally = Tally::default();
    replay_file("allowance/bodies", &mut tally);
    tally.assert_at_least(&[("allowance/bodies", 40), ("fetch", 40)]);
}

/// 状态码和 Retry-After 的各种组合。
#[test]
fn status_and_retry_after_snapshot() {
    let mut tally = Tally::default();
    replay_file("allowance/status", &mut tally);
    tally.assert_at_least(&[("allowance/status", 40), ("fetch", 40)]);
}

/// 多步轨迹：缓存命中 / 退避、额度偏低、周期采样、改阈值。
#[test]
fn traces_snapshot() {
    let mut tally = Tally::default();
    replay_file("allowance/traces", &mut tally);
    // 轨迹里要真的发生过请求（不能全是缓存命中 / 退避）
    tally.assert_at_least(&[
        ("allowance/traces", 3),
        ("fetch", 50),
        ("resolve", 100),
        ("note_period", 20),
        ("snapshot", 10),
        ("period", 10),
        ("set_threshold", 5),
    ]);
}
