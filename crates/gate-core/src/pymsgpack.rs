//! 复刻 `msgpack` 1.x（Cython 版）的 `unpackb(buf, raw=False)` 与 `packb(obj, use_bin_type=True)`。
//!
//! 解码自己写而不用 `rmpv::decode`：判定要与 Python 逐条一致——
//! - `strict_map_key=True`：map 的键只能是 str 或 bytes；
//! - `raw=False`：str 必须是合法 UTF-8（非法就整体失败，不是替换字符）；
//! - 容器（含空容器）嵌套超过 1024 层抛 `StackError`；
//! - 缓冲区必须恰好是一个值（多余字节 `ExtraData`，截断 `incomplete input`）；
//! - 扩展类型 -1 按 Timestamp 解码（长度只能 4 / 8 / 12，纳秒 ≤ 999999999），其余是 `ExtType`。
//!
//! 编码用 `rmpv`：它与 Python 一样总是选最短的整数 / 长度表示，浮点一律 float64、字符串走 str 家族。

use serde_json::Value;

use crate::pyobj::{DictKey, Obj, PyDict};

/// msgpack C 解包器的容器栈深（`MSGPACK_EMBED_STACK_SIZE`）。
pub const MAX_NESTING: usize = 1024;

/// 任何一种解码失败。图片事件流只关心「能不能解」，所以不细分（Python 里它们都是 ValueError 的子类）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("msgpack 解码失败")]
pub struct UnpackError;

/// 解码中的一层容器。
enum Open {
    Arr {
        items: Vec<Obj>,
        remaining: usize,
    },
    Map {
        dict: PyDict,
        remaining: usize,
        key: Option<DictKey>,
    },
}

/// 头部读完后的结果：一个完整的值，或者一个非空容器的开头。
enum Item {
    Val(Obj),
    Arr(usize),
    Map(usize),
}

/// `msgpack.unpackb(buf, raw=False)`（迭代实现：1024 层嵌套也不会深递归）。
pub fn unpackb(buf: &[u8]) -> Result<Obj, UnpackError> {
    let mut r = Reader { b: buf, pos: 0 };
    let mut stack: Vec<Open> = Vec::new();
    loop {
        let mut value = match r.item()? {
            Item::Val(v) => v,
            Item::Arr(n) => {
                // 空容器也先过栈深检查（C 解包器的 start_container 先判 top 再处理 count == 0）
                if stack.len() >= MAX_NESTING {
                    return Err(UnpackError); // StackError
                }
                if n == 0 {
                    Obj::List(Vec::new())
                } else {
                    let cap = n.min(r.b.len() - r.pos);
                    stack.push(Open::Arr {
                        items: Vec::with_capacity(cap),
                        remaining: n,
                    });
                    continue;
                }
            }
            Item::Map(n) => {
                if stack.len() >= MAX_NESTING {
                    return Err(UnpackError);
                }
                if n == 0 {
                    Obj::Dict(PyDict::new())
                } else {
                    stack.push(Open::Map {
                        dict: PyDict::new(),
                        remaining: n,
                        key: None,
                    });
                    continue;
                }
            }
        };
        // 把值挂到父容器上；容器填满就继续往上挂
        loop {
            let Some(top) = stack.last_mut() else {
                if r.pos != buf.len() {
                    return Err(UnpackError); // ExtraData
                }
                return Ok(value);
            };
            match top {
                Open::Arr { items, remaining } => {
                    items.push(value);
                    *remaining -= 1;
                    if *remaining > 0 {
                        break;
                    }
                    value = Obj::List(std::mem::take(items));
                }
                Open::Map {
                    dict,
                    remaining,
                    key,
                } => {
                    match key.take() {
                        None => {
                            // 刚读完键：strict_map_key 只允许 str / bytes
                            *key = Some(match value {
                                Obj::Str(ref mut s) => DictKey::Str(std::mem::take(s)),
                                Obj::Bytes(ref mut b) => DictKey::Bytes(std::mem::take(b)),
                                _ => return Err(UnpackError),
                            });
                            break;
                        }
                        Some(k) => dict.insert(k, value),
                    }
                    *remaining -= 1;
                    if *remaining > 0 {
                        break;
                    }
                    value = Obj::Dict(std::mem::take(dict));
                }
            }
            stack.pop();
        }
    }
}

struct Reader<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], UnpackError> {
        let end = self.pos.checked_add(n).ok_or(UnpackError)?;
        let s = self.b.get(self.pos..end).ok_or(UnpackError)?;
        self.pos = end;
        Ok(s)
    }

    fn u8(&mut self) -> Result<u8, UnpackError> {
        Ok(self.take(1)?[0])
    }
    fn be16(&mut self) -> Result<u16, UnpackError> {
        let s = self.take(2)?;
        Ok(u16::from_be_bytes([s[0], s[1]]))
    }
    fn be32(&mut self) -> Result<u32, UnpackError> {
        let s = self.take(4)?;
        Ok(u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
    }
    fn be64(&mut self) -> Result<u64, UnpackError> {
        let s = self.take(8)?;
        let mut a = [0u8; 8];
        a.copy_from_slice(s);
        Ok(u64::from_be_bytes(a))
    }

    fn string(&mut self, n: usize) -> Result<Obj, UnpackError> {
        let s = self.take(n)?;
        let text = std::str::from_utf8(s).map_err(|_| UnpackError)?;
        Ok(Obj::Str(text.to_string()))
    }

    fn ext(&mut self, n: usize) -> Result<Obj, UnpackError> {
        let code = self.u8()? as i8;
        let data = self.take(n)?;
        if code != -1 {
            return Ok(Obj::Ext {
                code,
                data: data.to_vec(),
            });
        }
        // Timestamp.from_bytes
        let (seconds, nanoseconds) = match data.len() {
            4 => (
                i64::from(u32::from_be_bytes([data[0], data[1], data[2], data[3]])),
                0u32,
            ),
            8 => {
                let mut a = [0u8; 8];
                a.copy_from_slice(data);
                let v = u64::from_be_bytes(a);
                ((v & 0x3_ffff_ffff) as i64, (v >> 34) as u32)
            }
            12 => {
                let ns = u32::from_be_bytes([data[0], data[1], data[2], data[3]]);
                let mut a = [0u8; 8];
                a.copy_from_slice(&data[4..]);
                (i64::from_be_bytes(a), ns)
            }
            _ => return Err(UnpackError),
        };
        if nanoseconds > 999_999_999 {
            return Err(UnpackError);
        }
        Ok(Obj::Timestamp {
            seconds,
            nanoseconds,
        })
    }

    /// 读一个值的头部（标量读完整，非空容器只读到长度）。
    fn item(&mut self) -> Result<Item, UnpackError> {
        let m = self.u8()?;
        let arr = Item::Arr;
        let map = Item::Map;
        Ok(Item::Val(match m {
            0x00..=0x7f => Obj::Int(i128::from(m)),
            0x80..=0x8f => return Ok(map(usize::from(m & 0x0f))),
            0x90..=0x9f => return Ok(arr(usize::from(m & 0x0f))),
            0xa0..=0xbf => return Ok(Item::Val(self.string(usize::from(m & 0x1f))?)),
            0xc0 => Obj::None,
            0xc1 => return Err(UnpackError),
            0xc2 => Obj::Bool(false),
            0xc3 => Obj::Bool(true),
            0xc4 => {
                let n = usize::from(self.u8()?);
                Obj::Bytes(self.take(n)?.to_vec())
            }
            0xc5 => {
                let n = usize::from(self.be16()?);
                Obj::Bytes(self.take(n)?.to_vec())
            }
            0xc6 => {
                let n = self.be32()? as usize;
                Obj::Bytes(self.take(n)?.to_vec())
            }
            0xc7 => {
                let n = usize::from(self.u8()?);
                return Ok(Item::Val(self.ext(n)?));
            }
            0xc8 => {
                let n = usize::from(self.be16()?);
                return Ok(Item::Val(self.ext(n)?));
            }
            0xc9 => {
                let n = self.be32()? as usize;
                return Ok(Item::Val(self.ext(n)?));
            }
            0xca => Obj::Float(f64::from(f32::from_bits(self.be32()?))),
            0xcb => Obj::Float(f64::from_bits(self.be64()?)),
            0xcc => Obj::Int(i128::from(self.u8()?)),
            0xcd => Obj::Int(i128::from(self.be16()?)),
            0xce => Obj::Int(i128::from(self.be32()?)),
            0xcf => Obj::Int(i128::from(self.be64()?)),
            0xd0 => Obj::Int(i128::from(self.u8()? as i8)),
            0xd1 => Obj::Int(i128::from(self.be16()? as i16)),
            0xd2 => Obj::Int(i128::from(self.be32()? as i32)),
            0xd3 => Obj::Int(i128::from(self.be64()? as i64)),
            0xd4 => return Ok(Item::Val(self.ext(1)?)),
            0xd5 => return Ok(Item::Val(self.ext(2)?)),
            0xd6 => return Ok(Item::Val(self.ext(4)?)),
            0xd7 => return Ok(Item::Val(self.ext(8)?)),
            0xd8 => return Ok(Item::Val(self.ext(16)?)),
            0xd9 => {
                let n = usize::from(self.u8()?);
                return Ok(Item::Val(self.string(n)?));
            }
            0xda => {
                let n = usize::from(self.be16()?);
                return Ok(Item::Val(self.string(n)?));
            }
            0xdb => {
                let n = self.be32()? as usize;
                return Ok(Item::Val(self.string(n)?));
            }
            0xdc => {
                let n = usize::from(self.be16()?);
                return Ok(arr(n));
            }
            0xdd => {
                let n = self.be32()? as usize;
                return Ok(arr(n));
            }
            0xde => {
                let n = usize::from(self.be16()?);
                return Ok(map(n));
            }
            0xdf => {
                let n = self.be32()? as usize;
                return Ok(map(n));
            }
            0xe0..=0xff => Obj::Int(i128::from(m as i8)),
        }))
    }
}

/// `msgpack.packb(payload, use_bin_type=True)`：payload 是 JSON 兼容的 dict。
/// 键顺序、整数最短编码、float64、str 家族与 Python 一致。
pub fn packb(payload: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    // 写入 Vec 不会失败
    rmpv::encode::write_value(&mut out, &to_rmpv(payload)).expect("写入 Vec 不会失败");
    out
}

fn to_rmpv(v: &Value) -> rmpv::Value {
    match v {
        Value::Null => rmpv::Value::Nil,
        Value::Bool(b) => rmpv::Value::Boolean(*b),
        Value::Number(n) => {
            if let Some(u) = n.as_u64() {
                rmpv::Value::Integer(u.into())
            } else if let Some(i) = n.as_i64() {
                rmpv::Value::Integer(i.into())
            } else {
                rmpv::Value::F64(n.as_f64().unwrap_or(f64::NAN))
            }
        }
        Value::String(s) => rmpv::Value::String(s.clone().into()),
        Value::Array(a) => rmpv::Value::Array(a.iter().map(to_rmpv).collect()),
        Value::Object(m) => rmpv::Value::Map(
            m.iter()
                .map(|(k, v)| (rmpv::Value::String(k.clone().into()), to_rmpv(v)))
                .collect(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn unpack_rules() {
        assert!(unpackb(&[0xc1]).is_err());
        assert!(unpackb(&[0x01, 0x02]).is_err());
        assert!(unpackb(&[]).is_err());
        assert!(unpackb(&[0x81, 0x01, 0x02]).is_err()); // int 键
        assert!(unpackb(&[0x81, 0xc4, 0x01, b'a', 0x02]).is_ok()); // bin 键
        assert!(unpackb(&[0xa2, 0xff, 0xfe]).is_err());
    }

    #[test]
    fn nesting_limit() {
        let mut ok = vec![0x91; MAX_NESTING];
        ok.push(0xc0);
        assert!(unpackb(&ok).is_ok());
        let mut bad = vec![0x91; MAX_NESTING + 1];
        bad.push(0xc0);
        assert!(unpackb(&bad).is_err());
    }

    #[test]
    fn pack_matches_python() {
        let v = json!({"a": 1, "b": [1.5, null, true], "c": -33, "d": u64::MAX, "e": i64::MIN, "f": 255, "g": 256});
        assert_eq!(
            hex::encode(packb(&v)),
            "87a16101a16293cb3ff8000000000000c0c3a163d0dfa164cfffffffffffffffffa165d38000000000000000a166ccffa167cd0100"
        );
    }
}
