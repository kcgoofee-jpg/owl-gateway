//! 快照 / 手写测试的公共小工具（各测试文件 `mod common;` 引入）。
#![allow(dead_code)]

use std::collections::BTreeMap;

use base64::Engine;
use gate_core::validation::ValidationResult;
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
pub fn outcome(r: ValidationResult<Value>) -> Value {
    r.unwrap_or_else(|e| json!({"err": e.type_name(), "msg": e.message()}))
}

/// JSON 对象的拷贝；不是对象就给空表。
pub fn obj(v: &Value) -> Map<String, Value> {
    v.as_object().cloned().unwrap_or_default()
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
