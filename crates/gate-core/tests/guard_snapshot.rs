//! guard.rs（账号保护）的快照。
//!
//! - 设置的读写和校验（`guard/settings`）；
//! - 安静时段的网格（`guard/grids`）；
//! - 操作轨迹（`guard/traces`）：`GuardCore` 在固定时钟 / 固定随机数 / 假库下执行的一串操作，每步记返回值、
//!   写库内容和执行后的全部内存状态（时间戳队列用 FNV-1a 摘要）。
//!
//! 常量、`GuardCore::new` 的默认值和 `parse_setting_float` 是手写的表。

mod common;

use common::{Tally, obj};
use gate_core::guard::{FIELDS, GuardCore, parse_setting_float};
use gate_core::pyext::PyErr;
use gate_core::reasons::Reason;
use gate_snapshot::snapshot::check;
use gate_snapshot::{Case, f};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

const M: &str = env!("CARGO_MANIFEST_DIR");

/// 执行后的全部内存状态：设置值（按 `FIELDS` 顺序）、每个账号的时间戳队列（条数 + 摘要）、在途数、排队表、
/// 序号、熔断。
fn state_json(g: &GuardCore) -> Value {
    let v: Vec<Value> = FIELDS.iter().map(|fl| json!(g.values[fl.name])).collect();
    let fl: Vec<Value> = g
        .image_inflight
        .iter()
        .map(|(k, n)| json!([k, n]))
        .collect();
    let e: Vec<Value> = g
        .entries
        .iter()
        .map(|e| json!([e.id, e.key, f(e.since), e.running]))
        .collect();
    json!({
        "v": v,
        "f": fl,
        "e": e,
        "q": g.seq,
        "b": [f(g.breaker_until), g.breaker_reason],
    })
}

fn pair(r: Option<Reason>) -> Value {
    r.map_or(Value::Null, |r| json!([r.text(), r.code()]))
}

/// 异常的记法：类型，ValueError 再带上面向用户的消息。
fn err_json(e: &PyErr) -> Value {
    match e {
        PyErr::Value(m) => json!({"err": e.type_name(), "msg": m}),
        _ => json!({"err": e.type_name()}),
    }
}

fn kv_writes(writes: &[(String, String)]) -> Value {
    Value::Array(writes.iter().map(|(k, v)| json!([k, v])).collect())
}

// ----------------------------------------------------------------------------- 手写的表

#[test]
fn constants() {
    // (名字, 默认, 下限, 上限)；说明文字在 describe 的快照里。
    // 站长 10-22 删掉了 daily_burst / account_hourly_cap / hourly_min / hourly_max / hourly_step，
    // 加了出图间隔 AIMD 的四个参数 interval_*（默认值 = gate_upstream::pace 的常量）
    // 10-23：出图间隔抖动 interval_jitter 删除；歇的下限 1 秒（原来写死的 SETTLE_GAP）
    let want: [(&str, i64, i64, i64); 5] = [
        ("account_daily_cap", 2400, 0, 20000),
        ("post_jitter_min", 1, 1, 30),
        ("post_jitter_max", 3, 1, 30),
        ("queue_per_member", 2, 1, 5),
        ("queue_per_account", 5, 0, 100),
    ];
    let got: Vec<(&str, i64, i64, i64)> = FIELDS
        .iter()
        .map(|fl| (fl.name, fl.default, fl.min, fl.max))
        .collect();
    assert_eq!(got, want);
}

#[test]
fn new_clamps_the_reset_hour_and_starts_from_defaults() {
    for (arg, want) in [
        (-5, 0),
        (-1, 0),
        (0, 0),
        (1, 1),
        (7, 7),
        (23, 23),
        (24, 23),
        (99, 23),
    ] {
        let g = GuardCore::new("Asia/Shanghai", arg);
        assert_eq!(g.reset_hour, want, "reset_hour_arg={arg}");
        for fl in &FIELDS {
            assert_eq!(g.values[fl.name], fl.default, "{}", fl.name);
        }
        assert_eq!(g.values_map().len(), FIELDS.len());
    }
}

#[test]
fn setting_float_table() {
    for (raw, want) in [
        (None, 0.0),
        (Some(""), 0.0),
        (Some("0"), 0.0),
        (Some("0.0"), 0.0),
        (Some("125"), 125.0),
        (Some("125.0"), 125.0),
        (Some(" 7 "), 7.0),
        (Some("1e3"), 1000.0),
        (Some("1_0"), 10.0),
        (Some("1790000000.123456"), 1790000000.123456),
        (Some("-5"), -5.0),
        (Some("５"), 5.0),
        (Some("inf"), f64::INFINITY),
        (Some("-inf"), f64::NEG_INFINITY),
    ] {
        assert_eq!(parse_setting_float(raw).unwrap(), want, "{raw:?}");
    }
    assert!(parse_setting_float(Some("nan")).unwrap().is_nan());
    for bad in ["abc", "0x1", "1,5"] {
        let e = parse_setting_float(Some(bad)).unwrap_err();
        assert_eq!(e.type_name(), "ValueError", "{bad}");
    }
}

// ----------------------------------------------------------------------------- 设置

#[test]
fn settings_snapshot() {
    let mut tally = Tally::default();
    check(M, "guard/settings", |c| match c.get("fn")?.as_str()? {
        "load" => {
            let mut g = GuardCore::new("Asia/Shanghai", 0);
            for (k, v) in obj(c.get("init")?) {
                g.values.insert(k, v.as_i64()?);
            }
            let stored: BTreeMap<String, String> = obj(c.get("stored")?)
                .into_iter()
                .map(|(k, v)| (k, v.as_str().unwrap().to_string()))
                .collect();
            let r = g.load_settings(&stored);
            tally.add("load");
            tally.add_if(r.is_err(), "load_overflow");
            // 出错时前面的字段已更新（与旧版一致），所以无论成败都记最终的值
            Some(json!({
                "err": r.err().map(|e| e.type_name()),
                "values": Value::Object(g.values_map()),
            }))
        }
        "save" => {
            let mut g = GuardCore::new("Asia/Shanghai", 0);
            let r = g.save(&obj(c.arg(0)));
            // to_settings 与 save 写库的格式一致：全部字段，按 FIELDS 顺序，整数不带 .0
            let all = g.to_settings();
            assert_eq!(all.len(), FIELDS.len());
            for (i, fl) in FIELDS.iter().enumerate() {
                assert_eq!(
                    all[i],
                    (format!("guard_{}", fl.name), g.values[fl.name].to_string())
                );
            }
            let values = Value::Object(g.values_map());
            Some(match r {
                Ok(saved) => {
                    tally.add("save");
                    json!({"writes": kv_writes(&saved.writes), "values": values})
                }
                Err(e) => {
                    tally.add("save_error");
                    let mut m = obj(&err_json(&e));
                    m.insert("values".into(), values);
                    Value::Object(m)
                }
            })
        }
        other => panic!("未知 {other}"),
    });
    tally.assert_at_least(&[
        ("load", 20),
        ("load_overflow", 3),
        ("save", 10),
        ("save_error", 20),
    ]);
}

// ----------------------------------------------------------------------------- 网格

// ----------------------------------------------------------------------------- 轨迹

/// 一条轨迹的重放：每步 `{"out" | "err", "writes", "state"}`。
fn replay(c: &Case, tally: &mut Tally) -> Vec<Value> {
    let init = c.get("init").unwrap();
    let mut g = GuardCore::new(
        init["tz"].as_str().unwrap(),
        init["reset_hour_arg"].as_i64().unwrap(),
    );
    // 已删除的设置（每小时上限 / 按时段放行）在旧用例里还有：忽略
    for (k, v) in obj(&init["values"]) {
        if FIELDS.iter().any(|fl| fl.name == k) {
            g.values.insert(k, v.as_i64().unwrap());
        }
    }
    let mut kv: BTreeMap<String, String> = obj(&init["kv"])
        .into_iter()
        .map(|(k, v)| (k, v.as_str().unwrap().to_string()))
        .collect();

    let mut results = Vec::new();
    for (i, step) in c
        .get("steps")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
    {
        let op = step["op"].as_str().unwrap();
        tally.add(op);
        let now = step["now"].as_f64().unwrap_or(0.0);
        let tok = step.get("token").and_then(Value::as_str).unwrap_or("");
        let mut writes: Vec<(String, String)> = Vec::new();

        // 结果：Ok(返回值) 或 Err(异常)
        let out: Result<Value, Value> = match op {
            // 安静时段与按账号的出图时间窗 10-23 删除：这几步不再适用
            // 出图间隔抖动 10-23 删除（出图节奏只剩出完后歇 N～M 秒）
            "record_starts" | "hourly_cap" | "in_quiet" | "hour_count" | "minutes_until_free"
            | "jitter" => {
                results.push(Value::Null);
                continue;
            }
            "token_block_reason" => Ok(pair(g.token_block_reason(
                tok,
                step["used"].as_f64().unwrap(),
                now,
            ))),
            "set_kv" => {
                let key = step["key"].as_str().unwrap().to_string();
                match step["value"].as_str() {
                    Some(v) => {
                        kv.insert(key, v.to_string());
                    }
                    None => {
                        kv.remove(&key);
                    }
                }
                results.push(Value::Null);
                continue;
            }
            "trip_breaker" => g
                .trip_breaker(
                    step["seconds"].as_f64().unwrap(),
                    step["reason"].as_str().unwrap(),
                    now,
                )
                .map(|()| Value::Null)
                .map_err(|e| err_json(&e)),
            "admit_image" => {
                // 用例给的是账号数：每个账号按「每账号排队数」算（不带上游估算）
                let accounts = step["accounts"].as_i64().unwrap().max(1) as usize;
                let cap = g.queue_cap(&vec![None; accounts]);
                Ok(pair(g.admit_image(step["key"].as_i64().unwrap(), cap, now)))
            }
            "mark_running" => {
                g.mark_running(step["key"].as_i64().unwrap(), step["entry_id"].as_i64());
                Ok(Value::Null)
            }
            "release_image" => {
                g.release_image(step["key"].as_i64().unwrap(), step["entry_id"].as_i64());
                Ok(Value::Null)
            }
            "queue_view" => Ok(g.queue_view(step["key"].as_i64())),
            "set_value" => {
                if !FIELDS
                    .iter()
                    .any(|fl| Some(fl.name) == step["name"].as_str())
                {
                    results.push(Value::Null); // 已删除的设置
                    continue;
                }
                g.values.insert(
                    step["name"].as_str().unwrap().to_string(),
                    step["value"].as_i64().unwrap(),
                );
                results.push(json!({"state": state_json(&g)}));
                continue;
            }
            "save" => match g.save(&obj(&step["body"])) {
                Ok(saved) => {
                    writes = saved.writes;
                    Ok(Value::Object(g.values_map()))
                }
                Err(e) => Err(err_json(&e)),
            },
            "describe" => Ok(g.describe()),
            other => panic!("第 {i} 步：未知操作 {other}"),
        };

        tally.add_if(out.is_err(), "error");
        let writes_json = kv_writes(&writes);
        // 写库内容（含顺序），并同步到假库
        for (k, v) in writes {
            kv.insert(k, v);
        }
        let mut m = Map::new();
        match out {
            Ok(v) => m.insert("out".into(), v),
            Err(e) => m.insert("err".into(), e),
        };
        m.insert("writes".into(), writes_json);
        m.insert("state".into(), state_json(&g));
        results.push(Value::Object(m));
    }
    results
}

#[test]
fn traces_snapshot() {
    let mut tally = Tally::default();
    check(M, "guard/traces", |c| {
        // 只剩已删除操作（429 减半 / 每日上调）的轨迹不再适用
        if c.get("steps")?.as_array()?.is_empty() {
            return None;
        }
        tally.add("trace");
        Some(Value::Array(replay(c, &mut tally)))
    });
    // 快照要真的覆盖到各个分支
    tally.assert_at_least(&[
        ("trace", 8),
        ("token_block_reason", 50),
        ("admit_image", 10),
        ("release_image", 5),
        ("describe", 5),
        ("save", 5),
        ("trip_breaker", 3),
    ]);
}
