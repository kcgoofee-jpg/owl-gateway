//! Python 对象的最小复刻：`json.loads` / `msgpack.unpackb` 的结果在 Python 里是任意对象，
//! `serde_json::Value` 装不下其中几类（NaN / Infinity、超大整数、bytes、msgpack 扩展类型），
//! 而「流里有没有这些东西」会影响接受 / 拒绝，所以图片事件流的判定必须在更完整的对象树上做。
//!
//! 只实现判定要用到的操作：真值（`bool(x)`）、`str(x)` / `repr(x)`、dict 查找。
//! 树可能很深（`json.loads` 的递归上限约 1 万层），所以 Drop / repr 都是迭代实现，不会爆栈。

use std::collections::HashMap;

use crate::{pyc, pyext};

/// dict 的键。JSON 只有 str 键；msgpack 在 `strict_map_key=True` 下只允许 str 与 bytes。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DictKey {
    Str(String),
    Bytes(Vec<u8>),
}

/// Python dict 语义：保持首次插入顺序；重复键保留原位置、值取最后一次（`d[k] = v`）。
#[derive(Debug, Clone, Default)]
pub struct PyDict {
    entries: Vec<(DictKey, Obj)>,
    /// 条目多了再建索引，小 dict 线性扫描更省。
    index: HashMap<DictKey, usize>,
}

const INDEX_THRESHOLD: usize = 16;

impl PyDict {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn position(&self, key: &DictKey) -> Option<usize> {
        if self.entries.len() < INDEX_THRESHOLD {
            self.entries.iter().position(|(k, _)| k == key)
        } else {
            self.index.get(key).copied()
        }
    }

    /// `d[key] = value`
    pub fn insert(&mut self, key: DictKey, value: Obj) {
        if let Some(i) = self.position(&key) {
            self.entries[i].1 = value;
            return;
        }
        if self.entries.len() + 1 == INDEX_THRESHOLD {
            for (i, (k, _)) in self.entries.iter().enumerate() {
                self.index.insert(k.clone(), i);
            }
        }
        if self.entries.len() + 1 >= INDEX_THRESHOLD {
            self.index.insert(key.clone(), self.entries.len());
        }
        self.entries.push((key, value));
    }

    /// `d.get("name")`：只匹配 str 键（bytes 键 `b"name"` 与 `"name"` 不相等）。
    pub fn get(&self, name: &str) -> Option<&Obj> {
        if self.entries.len() < INDEX_THRESHOLD {
            self.entries
                .iter()
                .find(|(k, _)| matches!(k, DictKey::Str(s) if s == name))
                .map(|(_, v)| v)
        } else {
            self.index
                .get(&DictKey::Str(name.to_string()))
                .map(|&i| &self.entries[i].1)
        }
    }

    /// `"name" in d`
    pub fn contains(&self, name: &str) -> bool {
        self.get(name).is_some()
    }

    pub fn iter(&self) -> impl Iterator<Item = &(DictKey, Obj)> {
        self.entries.iter()
    }
}

/// Python 对象。
#[derive(Debug, Clone, Default)]
pub enum Obj {
    #[default]
    None,
    Bool(bool),
    /// 能装进 i128 的整数（msgpack 的 u64 / i64、绝大多数 JSON 整数）。
    Int(i128),
    /// 超出 i128 的 JSON 大整数（Python 允许到 4300 位）。`digits` 无前导零。
    BigInt {
        neg: bool,
        digits: String,
    },
    Float(f64),
    Str(String),
    Bytes(Vec<u8>),
    List(Vec<Obj>),
    Dict(PyDict),
    /// msgpack 的 `ExtType(code, data)`。
    Ext {
        code: i8,
        data: Vec<u8>,
    },
    /// msgpack 的 `Timestamp(seconds, nanoseconds)`（扩展类型 -1）。
    Timestamp {
        seconds: i64,
        nanoseconds: u32,
    },
}

impl Drop for Obj {
    /// 迭代释放：1 万层嵌套的树用默认的递归 Drop 会爆栈。
    fn drop(&mut self) {
        let mut stack: Vec<Obj> = Vec::new();
        take_children(self, &mut stack);
        while let Some(mut o) = stack.pop() {
            take_children(&mut o, &mut stack);
        }
    }
}

fn take_children(o: &mut Obj, out: &mut Vec<Obj>) {
    match o {
        Obj::List(items) => out.append(items),
        Obj::Dict(d) => {
            d.index.clear();
            out.extend(d.entries.drain(..).map(|(_, v)| v));
        }
        _ => {}
    }
}

impl Obj {
    /// 十进制串 → Python int。`digits` 是无符号数字串（无前导零，除非就是 "0"）。
    pub fn int_from_decimal(neg: bool, digits: &str) -> Obj {
        // u128 最多 39 位；i128 绝对值上限 1.7e38，所以 ≤ 38 位一定放得下
        if digits.len() <= 38 {
            let v: i128 = digits.parse().expect("≤38 位十进制数必定放进 i128");
            return Obj::Int(if neg { -v } else { v });
        }
        let parsed = format!("{}{digits}", if neg { "-" } else { "" }).parse::<i128>();
        match parsed {
            Ok(v) => Obj::Int(v),
            Err(_) => Obj::BigInt {
                neg,
                digits: digits.to_string(),
            },
        }
    }

    /// `type(x) is int`（bool 不算）。
    pub fn is_int(&self) -> bool {
        matches!(self, Obj::Int(_) | Obj::BigInt { .. })
    }

    /// `bool(x)`
    pub fn truthy(&self) -> bool {
        match self {
            Obj::None => false,
            Obj::Bool(b) => *b,
            Obj::Int(i) => *i != 0,
            Obj::BigInt { .. } => true,
            Obj::Float(f) => *f != 0.0,
            Obj::Str(s) => !s.is_empty(),
            Obj::Bytes(b) => !b.is_empty(),
            Obj::List(l) => !l.is_empty(),
            Obj::Dict(d) => !d.is_empty(),
            // namedtuple（2 个元素）与 Timestamp 对象都为真
            Obj::Ext { .. } | Obj::Timestamp { .. } => true,
        }
    }

    /// `str(x)`：str 原样，其余同 `repr(x)`。
    pub fn py_str(&self) -> String {
        match self {
            Obj::Str(s) => s.clone(),
            other => other.py_repr(),
        }
    }

    /// `repr(x)`（迭代实现，深层嵌套不爆栈）。
    pub fn py_repr(&self) -> String {
        enum Task<'a> {
            Val(&'a Obj),
            Key(&'a DictKey),
            Lit(&'static str),
        }
        let mut out = String::new();
        let mut stack = vec![Task::Val(self)];
        while let Some(task) = stack.pop() {
            match task {
                Task::Lit(s) => out.push_str(s),
                Task::Key(DictKey::Str(s)) => out.push_str(&pyext::repr_str(s)),
                Task::Key(DictKey::Bytes(b)) => out.push_str(&bytes_repr(b)),
                Task::Val(Obj::List(items)) => {
                    out.push('[');
                    stack.push(Task::Lit("]"));
                    for (i, item) in items.iter().enumerate().rev() {
                        stack.push(Task::Val(item));
                        if i > 0 {
                            stack.push(Task::Lit(", "));
                        }
                    }
                }
                Task::Val(Obj::Dict(d)) => {
                    out.push('{');
                    stack.push(Task::Lit("}"));
                    for (i, (k, v)) in d.entries.iter().enumerate().rev() {
                        stack.push(Task::Val(v));
                        stack.push(Task::Lit(": "));
                        stack.push(Task::Key(k));
                        if i > 0 {
                            stack.push(Task::Lit(", "));
                        }
                    }
                }
                Task::Val(scalar) => out.push_str(&scalar_repr(scalar)),
            }
        }
        out
    }
}

fn scalar_repr(o: &Obj) -> String {
    match o {
        Obj::None => "None".into(),
        Obj::Bool(b) => pyc::bool_str(*b).into(),
        Obj::Int(i) => i.to_string(),
        Obj::BigInt { neg, digits } => format!("{}{digits}", if *neg { "-" } else { "" }),
        Obj::Float(f) => pyc::float_repr(*f),
        Obj::Str(s) => pyext::repr_str(s),
        Obj::Bytes(b) => bytes_repr(b),
        Obj::Ext { code, data } => format!("ExtType(code={code}, data={})", bytes_repr(data)),
        Obj::Timestamp {
            seconds,
            nanoseconds,
        } => format!("Timestamp(seconds={seconds}, nanoseconds={nanoseconds})"),
        Obj::List(_) | Obj::Dict(_) => unreachable!("容器由 py_repr 展开"),
    }
}

fn pick_quote(has_single: bool, has_double: bool) -> char {
    if has_single && !has_double { '"' } else { '\'' }
}

/// `repr(bytes)`
pub fn bytes_repr(b: &[u8]) -> String {
    let quote = pick_quote(b.contains(&b'\''), b.contains(&b'"'));
    let mut out = String::with_capacity(b.len() + 3);
    out.push('b');
    out.push(quote);
    for &x in b {
        match x {
            b'\\' => out.push_str("\\\\"),
            b'\t' => out.push_str("\\t"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            x if x as char == quote => {
                out.push('\\');
                out.push(x as char);
            }
            0x20..=0x7e => out.push(x as char),
            x => out.push_str(&format!("\\x{x:02x}")),
        }
    }
    out.push(quote);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repr_matches_python() {
        assert_eq!(pyext::repr_str("it's"), "\"it's\"");
        assert_eq!(pyext::repr_str("a'b\"c"), "'a\\'b\"c'");
        assert_eq!(pyext::repr_str("猫\n\u{7f}\u{a0}"), "'猫\\n\\x7f\\xa0'");
        assert_eq!(bytes_repr(b"a\x00'\xff"), "b\"a\\x00'\\xff\"");
        let mut d = PyDict::new();
        d.insert(
            DictKey::Str("k".into()),
            Obj::List(vec![Obj::Int(1), Obj::None]),
        );
        assert_eq!(Obj::Dict(d).py_repr(), "{'k': [1, None]}");
        assert_eq!(Obj::Float(f64::NAN).py_str(), "nan");
        assert_eq!(Obj::Dict(PyDict::new()).py_repr(), "{}");
    }

    #[test]
    fn dict_keeps_first_position_last_value() {
        let mut d = PyDict::new();
        for i in 0..40 {
            d.insert(DictKey::Str(format!("k{i}")), Obj::Int(i));
        }
        d.insert(DictKey::Str("k3".into()), Obj::Int(99));
        assert_eq!(d.len(), 40);
        assert!(matches!(d.get("k3"), Some(Obj::Int(99))));
        assert_eq!(
            d.iter().nth(3).map(|(k, _)| k.clone()),
            Some(DictKey::Str("k3".into()))
        );
        d.insert(DictKey::Bytes(b"k3".to_vec()), Obj::None);
        assert!(matches!(d.get("k3"), Some(Obj::Int(99))));
    }

    #[test]
    fn deep_tree_drops_without_stack_overflow() {
        let mut o = Obj::None;
        for _ in 0..200_000 {
            o = Obj::List(vec![o]);
        }
        drop(o);
    }
}
