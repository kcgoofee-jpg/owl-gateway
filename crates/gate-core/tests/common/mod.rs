//! 快照 / 手写测试的公共小工具（各测试文件 `mod common;` 引入）。
#![allow(dead_code)]

use std::collections::BTreeMap;

use base64::Engine;
use gate_core::pyext::PyResult;
use gate_core::pyobj::{DictKey, Obj};
use serde_json::{Map, Value, json};

/// 快照里的 bytes：`{"$b": "<base64>"}`。
pub fn bytes_of(v: &Value) -> Vec<u8> {
    let s = v
        .get("$b")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("不是 bytes 编码：{v}"));
    base64::engine::general_purpose::STANDARD
        .decode(s)
        .unwrap_or_else(|e| panic!("base64 解码失败：{e}"))
}

pub fn b64(data: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(data)
}

/// 异常也记进输出：`{"err": 类型, "msg": 消息}`，成功原样。
pub fn outcome(r: PyResult<Value>) -> Value {
    r.unwrap_or_else(|e| json!({"err": e.type_name(), "msg": e.message()}))
}

/// JSON 对象的拷贝；不是对象就给空表。
pub fn obj(v: &Value) -> Map<String, Value> {
    v.as_object().cloned().unwrap_or_default()
}

/// Python 对象 → 带类型标记的 JSON（与 streams.py 的 `jenc_loads` 一一对应）：
/// `["n"]` `["b", bool]` `["i", "十进制串"]` `["f", float]` `["s", str]` `["B", b64]`
/// `["l", [...]]` `["d", [[key, value], ...]]` `["x", code, b64]` `["t", seconds, nanoseconds]`。
/// 递归实现，只用于深度不大的结果。
pub fn tag(o: &Obj) -> Value {
    match o {
        Obj::None => json!(["n"]),
        Obj::Bool(b) => json!(["b", b]),
        Obj::Int(i) => json!(["i", i.to_string()]),
        Obj::BigInt { neg, digits } => {
            json!(["i", format!("{}{digits}", if *neg { "-" } else { "" })])
        }
        Obj::Float(x) => json!(["f", gate_snapshot::f(*x)]),
        Obj::Str(s) => json!(["s", s]),
        Obj::Bytes(b) => json!(["B", b64(b)]),
        Obj::List(items) => json!(["l", items.iter().map(tag).collect::<Vec<_>>()]),
        Obj::Dict(d) => {
            let pairs: Vec<Value> = d
                .iter()
                .map(|(k, v)| {
                    let key = match k {
                        DictKey::Str(s) => json!(["s", s]),
                        DictKey::Bytes(b) => json!(["B", b64(b)]),
                    };
                    json!([key, tag(v)])
                })
                .collect();
            json!(["d", pairs])
        }
        Obj::Ext { code, data } => json!(["x", code, b64(data)]),
        Obj::Timestamp {
            seconds,
            nanoseconds,
        } => json!(["t", seconds, nanoseconds]),
    }
}

/// 容器嵌套深度（迭代，深层树也安全）。
pub fn depth_of(o: &Obj) -> usize {
    let mut best = 0;
    let mut stack = vec![(o, 0usize)];
    while let Some((x, d)) = stack.pop() {
        match x {
            Obj::List(items) => {
                best = best.max(d + 1);
                stack.extend(items.iter().map(|c| (c, d + 1)));
            }
            Obj::Dict(m) => {
                best = best.max(d + 1);
                stack.extend(m.iter().map(|(_, c)| (c, d + 1)));
            }
            _ => {}
        }
    }
    best
}

/// 分类计数：快照被误删成空壳时，这里的「每类至少 k 条」断言会先报出来（代替旧测试里的「用例数 > N」）。
#[derive(Default)]
pub struct Tally(BTreeMap<String, usize>);

impl Tally {
    pub fn add(&mut self, kind: &str) {
        *self.0.entry(kind.to_string()).or_default() += 1;
    }

    /// `cond` 为真时计一次。
    pub fn add_if(&mut self, cond: bool, kind: &str) {
        if cond {
            self.add(kind);
        }
    }

    pub fn count(&self, kind: &str) -> usize {
        self.0.get(kind).copied().unwrap_or(0)
    }

    /// 每个 `(类别, k)` 至少出现 k 次。
    #[track_caller]
    pub fn assert_at_least(&self, wants: &[(&str, usize)]) {
        for (kind, min) in wants {
            assert!(
                self.count(kind) >= *min,
                "快照里 {kind} 只有 {} 条，应至少 {min} 条：{:?}",
                self.count(kind),
                self.0
            );
        }
    }
}
