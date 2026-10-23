//! site_flags / features / bot_config 的快照。
//!
//! - 站点设置：登记表、解析、默认值、读取、编码（`flags/site_*`）；
//! - 功能开关：常量、纯函数、准入判定（`flags/features_*`）；
//! - 机器人心跳（`bot_config`）：只剩白名单字段和在线判断，写成手写的表。

mod common;

use common::{Tally, obj};
use gate_core::bot_config::{self, BotConfigError};
use gate_core::features::{self, FeaturesError, VibeAnlas};
use gate_core::pyval;
use gate_core::site_flags::{self, ALL, Kind, Spec, SpecDefault};
use gate_snapshot::snapshot::check;
use serde_json::{Map, Value, json};

const M: &str = env!("CARGO_MANIFEST_DIR");

fn leak(s: &str) -> &'static str {
    Box::leak(s.to_string().into_boxed_str())
}

/// 快照里的 Spec：`{"ref": key}` 取登记表，`{"inline": {...}}` 现场构造（覆盖登记表里没有的类型组合）。
/// 引用的键不在登记表里（设置被删了）返回 None。
fn spec_of(v: &Value) -> Option<Spec> {
    if let Some(k) = v.get("ref").and_then(Value::as_str) {
        return ALL.iter().find(|s| s.key == k).map(|s| **s);
    }
    let d = &v["inline"];
    let kind = match d["kind"].as_str()? {
        "flag" => Kind::Flag,
        "int" => Kind::Int,
        "float" => Kind::Float,
        "str" => Kind::Str,
        "choice" => Kind::Choice,
        other => panic!("未知类型 {other}"),
    };
    let default = match &d["default"] {
        Value::Bool(b) => SpecDefault::Bool(*b),
        Value::Number(n) if n.is_f64() => SpecDefault::Float(n.as_f64()?),
        Value::Number(n) => SpecDefault::Int(n.as_i64()?),
        Value::String(s) => SpecDefault::Str(leak(s)),
        other => panic!("不支持的默认值 {other}"),
    };
    let choices: Vec<&'static str> = d["choices"]
        .as_array()?
        .iter()
        .map(|c| leak(c.as_str().unwrap()))
        .collect();
    Some(Spec {
        key: leak(d["key"].as_str()?),
        kind,
        default,
        owner: leak(d["owner"].as_str()?),
        doc: leak(d["doc"].as_str()?),
        lo: d["lo"].as_i64(),
        hi: d["hi"].as_i64(),
        choices: Box::leak(choices.into_boxed_slice()),
    })
}

fn spec_json(s: &Spec) -> Value {
    json!({
        "key": s.key,
        "kind": s.kind.as_str(),
        "default": match s.default {
            SpecDefault::Bool(b) => json!(b),
            SpecDefault::Int(i) => json!(i),
            SpecDefault::Float(x) => pyval::float(x),
            SpecDefault::Str(t) => json!(t),
        },
        "owner": s.owner,
        "doc": s.doc,
        "lo": s.lo,
        "hi": s.hi,
        "choices": s.choices,
    })
}

// ---------------------------------------------------------------- site_flags

/// 默认值固定写在登记表里的设置（键, 默认值）。
const FIXED_DEFAULTS: [(&str, &str); 3] = [
    ("global_daily_v5", "150"),
    ("global_daily_anlas", "200"),
    ("global_monthly_anlas", "10000.0"),
];

#[test]
fn fixed_defaults_come_from_the_registry() {
    for (key, want) in FIXED_DEFAULTS {
        let spec = **ALL.iter().find(|s| s.key == key).unwrap();
        let want: Value = serde_json::from_str(want).unwrap();
        assert_eq!(site_flags::default_of(&spec), want, "{key}");
        assert_eq!(site_flags::get_from_raw(&spec, None), want, "{key}");
        assert_eq!(site_flags::get_from_raw(&spec, Some("abc")), want, "{key}");
        assert_eq!(
            site_flags::get_from_raw(&spec, Some("90")),
            if want.is_f64() {
                json!(90.0)
            } else {
                json!(90)
            },
            "{key}"
        );
    }
}

/// 登记表：每个设置一条（键、类型、默认值、归属、说明、范围、可选值），再加完整的键顺序。
#[test]
fn site_registry_snapshot() {
    let mut specs = 0;
    check(M, "flags/site_registry", |c| {
        match c.get("fn")?.as_str()? {
            "spec" => {
                let key = c.get("key")?.as_str()?;
                let s = ALL.iter().find(|s| s.key == key)?;
                specs += 1;
                Some(spec_json(s))
            }
            "all_keys" => Some(json!(ALL.iter().map(|s| s.key).collect::<Vec<_>>())),
            other => panic!("未知 {other}"),
        }
    });
    // 新增 / 删除设置后要同步这份快照（`UPDATE_SNAPSHOT=1`，新增的键手工追加一行 spec 用例）
    assert_eq!(specs, ALL.len(), "登记表条数和快照里的 spec 条数不一致");
}

#[test]
fn site_parse_snapshot() {
    let mut n = 0;
    check(M, "flags/site_parse", |c| {
        let spec = spec_of(c.get("spec")?)?;
        n += 1;
        Some(site_flags::parse(&spec, c.arg(0), c.arg(1).clone()))
    });
    assert!(n >= 50, "用例太少：{n}");
}

#[test]
fn site_get_snapshot() {
    let mut n = 0;
    check(M, "flags/site_get", |c| {
        let spec = spec_of(c.get("spec")?)?;
        let raw = c
            .get("present")?
            .as_bool()?
            .then(|| c.get("raw").and_then(Value::as_str))
            .flatten();
        n += 1;
        Some(site_flags::get_from_raw(&spec, raw))
    });
    assert!(n >= 50, "用例太少：{n}");
}

#[test]
fn site_encode_snapshot() {
    let mut tally = Tally::default();
    check(M, "flags/site_encode", |c| {
        let spec = spec_of(c.get("spec")?)?;
        Some(match site_flags::encode(&spec, c.arg(0)) {
            Ok(s) => {
                tally.add("ok");
                Value::String(s)
            }
            Err(e) => {
                tally.add("error");
                json!({"err": "ValueError", "msg": e.0})
            }
        })
    });
    tally.assert_at_least(&[("ok", 20), ("error", 20)]);
}

// ---------------------------------------------------------------- features

fn str_list(v: &Value) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_str().unwrap().to_string())
        .collect()
}

fn pairs(p: &[(&str, &str)]) -> Value {
    Value::Array(p.iter().map(|(a, b)| json!([a, b])).collect())
}

#[test]
fn features_constants() {
    assert_eq!(
        pairs(&features::FEATURES),
        json!([
            ["image", "文生图"],
            ["upscale", "放大"],
            ["augment", "导演工具（线稿、上色等）"],
            ["vibe", "Vibe 编码"],
            ["tags", "标签补全"],
        ])
    );
    assert_eq!(features::GLOBAL_KEY, "features_global");
    assert_eq!(
        pairs(&features::ANLAS_FEATURES),
        json!([
            ["vibe", "每次编码参考图约 2 Anlas"],
            ["upscale", "按图片大小扣 Anlas"],
            ["augment", "按图片大小扣 Anlas"],
        ])
    );
    assert_eq!(
        pairs(&features::KIND_FEATURE),
        json!([
            ["image", "image"],
            ["image_stream", "image"],
            ["upscale", "upscale"],
            ["augment-image", "augment"],
            ["vibe_encode", "vibe"],
            ["tags", "tags"],
        ])
    );
}

#[test]
fn features_pure_snapshot() {
    let mut tally = Tally::default();
    check(M, "flags/features_pure", |c| {
        let name = c.get("fn")?.as_str()?;
        tally.add(name);
        Some(match name {
            "kinds_for" => json!(features::kinds_for(c.arg(0).as_str()?)),
            "parse_list" => features::parse_list(c.arg(0)).map_or(Value::Null, |l| json!(l)),
            "normalize" => json!(features::normalize(str_list(c.arg(0)))),
            "dump" => {
                let names = (!c.arg(0).is_null()).then(|| str_list(c.arg(0)));
                features::dump(names).map_or(Value::Null, Value::String)
            }
            "key_features" => features::key_features(c.arg(0)).map_or(Value::Null, |l| json!(l)),
            "global_flags" => match features::parse_global_flags(c.arg(0).as_str()) {
                // 按 FEATURES 的顺序
                Ok(f) => Value::Object(
                    features::FEATURES
                        .iter()
                        .map(|(n, _)| ((*n).to_string(), json!(f[n])))
                        .collect(),
                ),
                Err(FeaturesError::Attribute(m)) => json!({"err": "AttributeError", "msg": m}),
                Err(other) => panic!("意外的错误 {other}"),
            },
            other => panic!("未知 {other}"),
        })
    });
    tally.assert_at_least(&[
        ("kinds_for", 5),
        ("parse_list", 10),
        ("normalize", 5),
        ("dump", 5),
        ("key_features", 5),
        ("global_flags", 10),
    ]);
}

/// 准入判定：某个 Key 用某个功能是否被拦（全站开关、Key 自己的功能表、Vibe 的 Anlas 额度）。
#[test]
fn features_check_snapshot() {
    let mut tally = Tally::default();
    check(M, "flags/features_check", |c| {
        let kv = c.arg(0);
        let key = c.arg(1);
        let name = c.arg(2).as_str()?;
        let global = kv.get(features::GLOBAL_KEY).and_then(Value::as_str);
        let needs = features::needs_vibe_site_flags(global, key, name);
        // 全站每日 Anlas 额度：服务层按登记表里的设置读，需要时才读
        let vibe = needs.then(|| {
            let spec = ALL.iter().find(|s| s.key == "global_daily_anlas").unwrap();
            let cap = site_flags::get_from_raw(
                spec,
                kv.get("global_daily_anlas").and_then(Value::as_str),
            );
            VibeAnlas {
                daily_cap: cap.as_i64().unwrap(),
            }
        });
        tally.add("all");
        tally.add_if(needs, "needs_vibe");
        Some(match features::check_decision(global, key, name, vibe) {
            Ok(decision) => {
                tally.add(if decision.is_some() { "deny" } else { "allow" });
                json!({"out": decision, "needs_vibe": needs})
            }
            Err(e) => {
                tally.add("error");
                json!({"err": match e {
                    FeaturesError::Attribute(_) => "AttributeError",
                    FeaturesError::Key(_) => "KeyError",
                    FeaturesError::MissingVibe => "MissingVibe",
                }})
            }
        })
    });
    // 快照要真的覆盖到各个分支
    tally.assert_at_least(&[
        ("all", 50),
        ("deny", 1),
        ("allow", 10),
        ("error", 3),
        ("needs_vibe", 5),
    ]);
}

// ---------------------------------------------------------------- bot_config
//
// 机器人只剩心跳：白名单字段、`seen_at`、3 分钟没心跳算离线。

#[test]
fn bot_status_key_and_window() {
    assert_eq!(bot_config::STATUS_KEY, "bot_status");
    assert_eq!(bot_config::ONLINE_WINDOW, 180.0);
}

#[test]
fn bot_heartbeat_keeps_whitelisted_fields_only() {
    // 白名单：user guild latency_ms ready_at version；ai_* 和别的键都被丢弃，seen_at 是上报时间
    let status = json!({
        "version": "猫x", "guild": 0.1, "ai_ready": "奶妹", "ai_today": 3, "user": null,
        "seen_at": 3.75, "ready_at": "猫x", "token": [1, "a"], "event": {"kind": "like"},
        "latency_ms": 42
    });
    let stored = bot_config::status_to_store(status.as_object().unwrap(), 1790947275.123456);
    assert_eq!(
        stored,
        r#"{"user": null, "guild": 0.1, "latency_ms": 42, "ready_at": "猫x", "version": "猫x", "seen_at": 1790947275.123456}"#
    );
    // 空心跳也有 seen_at；字段顺序固定，不看入参的顺序
    assert_eq!(
        bot_config::status_to_store(&Map::new(), 1000.0),
        r#"{"seen_at": 1000.0}"#
    );
}

#[test]
fn bot_snapshot_online_follows_the_window() {
    let raw = bot_config::status_to_store(&obj(&json!({"user": "naimei"})), 1000.0);
    let at = |dt: f64| bot_config::snapshot(Some(&raw), 1000.0 + dt).unwrap();
    for (dt, online) in [(0.0, true), (179.9, true), (180.0, false), (3600.0, false)] {
        assert_eq!(
            at(dt),
            json!({"status": {"user": "naimei", "seen_at": 1000.0, "online": online}}),
            "dt={dt}"
        );
    }
    // 还没收到过心跳
    for empty in [None, Some("")] {
        assert_eq!(
            bot_config::snapshot(empty, 1000.0).unwrap(),
            json!({"status": {"online": false}})
        );
    }
    // seen_at 为假（0 / 空串 / false / null）算离线，不报错
    for seen in ["0", "\"\"", "false", "null"] {
        let raw = format!(r#"{{"seen_at": {seen}}}"#);
        let got = bot_config::snapshot(Some(&raw), 1000.0).unwrap();
        assert_eq!(got["status"]["online"], false, "seen_at={seen}");
    }
}

#[test]
fn bot_snapshot_rejects_broken_status() {
    for (raw, kind) in [
        ("not json", "BadJson"),
        ("{", "BadJson"),
        ("[1]", "WrongType"),
        ("\"abc\"", "WrongType"),
        ("null", "WrongType"),
        (r#"{"seen_at": "x"}"#, "Type"),
        (r#"{"seen_at": [1]}"#, "Type"),
    ] {
        let err = bot_config::snapshot(Some(raw), 0.0).unwrap_err();
        let got = match err {
            BotConfigError::BadJson(_) => "BadJson",
            BotConfigError::WrongType(_) => "WrongType",
            BotConfigError::Type(_) => "Type",
        };
        assert_eq!(got, kind, "{raw}");
    }
}
