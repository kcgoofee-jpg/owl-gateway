//! 测试快照：把当前 Rust 输出存成 `tests/snapshots/<套件>/<名字>.jsonl`，一行一个用例 `{"in": ..., "out": ...}`。
//!
//! - [`snapshot`]：`check` / `Suite`，默认模式逐行比对，`UPDATE_SNAPSHOT=1` 重写（CI 里禁止更新）；
//! - 本文件：用例 [`Case`]（数据来自 `"in"`）、JSON 差异定位 [`diff`]、特殊浮点编码 [`f`] / [`as_f64`]。
//!
//! 编码约定：
//! - int → JSON 整数，float → 带小数点或指数的 JSON 数，二者类型不混；
//! - 非有限浮点 → `{"$f": "inf" | "-inf" | "nan"}`；
//! - bytes → `{"$b": "<base64>"}`；
//! - dict 的非 str 键 → `{"$map": [[k, v], ...]}`。
//!
//! 3.0 上线期间这里还有一套读 2.x Python 冻结夹具、按 Python 语义比对的 golden API，第四批对拍退役后删除
//! （生成脚本和旧代码在 git 标签 `py-final`，逐条偏差表在标签 `v3-deviations-final`）。

pub mod snapshot;

use serde_json::Value;

/// 一个用例。
#[derive(Debug, Clone)]
pub struct Case {
    pub line: usize,
    pub raw: Value,
}

impl Case {
    pub fn args(&self) -> &[Value] {
        self.raw
            .get("args")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }
    pub fn arg(&self, i: usize) -> &Value {
        &self.args()[i]
    }
    pub fn kwarg(&self, name: &str) -> Option<&Value> {
        self.raw.get("kwargs")?.get(name)
    }
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.raw.get(key)
    }
}

/// 解码特殊浮点：`{"$f": "inf"}` → f64。普通数字原样返回。
pub fn as_f64(v: &Value) -> Option<f64> {
    if let Some(f) = v.as_f64() {
        return Some(f);
    }
    match v.get("$f").and_then(Value::as_str)? {
        "inf" => Some(f64::INFINITY),
        "-inf" => Some(f64::NEG_INFINITY),
        "nan" => Some(f64::NAN),
        _ => None,
    }
}

/// 比较两个 JSON：int 与 float 类型必须一致；浮点逐位相等（NaN 视为相等）；对象的键顺序也要一致。
/// 不一致时返回第一处差异的路径说明。
pub fn diff(actual: &Value, expected: &Value) -> Option<String> {
    fn walk(a: &Value, e: &Value, path: &str) -> Option<String> {
        // 特殊浮点
        if e.get("$f").is_some() || a.get("$f").is_some() {
            return match (as_f64(a), as_f64(e)) {
                (Some(x), Some(y)) if x.to_bits() == y.to_bits() || (x.is_nan() && y.is_nan()) => {
                    None
                }
                _ => Some(format!("{path}: 实际 {a} ≠ 期望 {e}")),
            };
        }
        match (a, e) {
            (Value::Number(x), Value::Number(y)) => {
                let xf = x.is_f64();
                let yf = y.is_f64();
                if xf != yf {
                    return Some(format!(
                        "{path}: 类型不同（实际 {} {x}，期望 {} {y}）",
                        if xf { "float" } else { "int" },
                        if yf { "float" } else { "int" }
                    ));
                }
                if xf {
                    let (p, q) = (x.as_f64().unwrap(), y.as_f64().unwrap());
                    if p.to_bits() != q.to_bits() {
                        return Some(format!("{path}: 实际 {p:?} ≠ 期望 {q:?}"));
                    }
                    None
                } else if x != y {
                    Some(format!("{path}: 实际 {x} ≠ 期望 {y}"))
                } else {
                    None
                }
            }
            (Value::Array(xs), Value::Array(ys)) => {
                if xs.len() != ys.len() {
                    return Some(format!("{path}: 长度 {} ≠ {}", xs.len(), ys.len()));
                }
                xs.iter()
                    .zip(ys)
                    .enumerate()
                    .find_map(|(i, (x, y))| walk(x, y, &format!("{path}[{i}]")))
            }
            (Value::Object(xm), Value::Object(ym)) => {
                let xk: Vec<_> = xm.keys().collect();
                let yk: Vec<_> = ym.keys().collect();
                if xk != yk {
                    return Some(format!("{path}: 键顺序 {xk:?} ≠ {yk:?}"));
                }
                for k in ym.keys() {
                    if !xm.contains_key(k) {
                        return Some(format!("{path}: 缺少键 {k:?}"));
                    }
                }
                for k in xm.keys() {
                    if !ym.contains_key(k) {
                        return Some(format!("{path}: 多出键 {k:?}"));
                    }
                }
                ym.iter()
                    .find_map(|(k, y)| walk(&xm[k], y, &format!("{path}.{k}")))
            }
            _ if a == e => None,
            _ => Some(format!("{path}: 实际 {a} ≠ 期望 {e}")),
        }
    }
    walk(actual, expected, "$")
}

/// 把 f64 编成与夹具同样的 JSON（非有限值用 `$f` 标记）。
pub fn f(x: f64) -> Value {
    if x.is_finite() {
        serde_json::Number::from_f64(x)
            .map(Value::Number)
            .unwrap_or(Value::Null)
    } else if x.is_nan() {
        serde_json::json!({"$f": "nan"})
    } else if x > 0.0 {
        serde_json::json!({"$f": "inf"})
    } else {
        serde_json::json!({"$f": "-inf"})
    }
}
