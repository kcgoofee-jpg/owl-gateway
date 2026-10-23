//! `json_body` / `pyloads`：请求体 JSON 的接受 / 拒绝，与旧网关（Python 3.12.14 的 `json.loads`）逐条一致。
//!
//! 期望值都是 CPython 3.12.14 里实际跑出来的结果，不是拿 Rust 实现反推的：
//! - 接受的输入给出 `repr(json.loads(raw))`；
//! - 拒绝的输入给出错误类别（`JSONDecodeError` → `Syntax`，`UnicodeDecodeError` → `Decode`，……）；
//! - 请求体层面给出 `json.dumps(obj, ensure_ascii=False, separators=(",", ":"))`，键顺序也要一致：它会原样转发给上游。
//!
//! 有三类输入 `serde_json::Value` 装不下（孤立代理 / 超出 u64 的整数 / 溢出浮点）：照样接受，但打 `Lossy` 标记。
//! 它们进入 `Value` 之后的取值是模块文档里的约定，不是 Python 的结果，单独成表。

use gate_core::json_body::{
    self, BodyJsonError, Lossy, MAX_JSON_DEPTH, MSG_INVALID_JSON, MSG_NOT_OBJECT,
    THREAD_PARSE_BYTES,
};
use gate_core::pyjson;
use gate_core::pyloads::{self, JsonError, MAX_INT_DIGITS, Options, RECURSION_DEPTH};

const fn lossy(lone_surrogate: bool, big_int: bool, float_overflow: bool) -> Lossy {
    Lossy {
        lone_surrogate,
        big_int,
        float_overflow,
    }
}

/// 没有丢信息 / 有孤立代理 / 有超出 u64 的整数 / 有溢出浮点。
const NONE: Lossy = lossy(false, false, false);
const LONE: Lossy = lossy(true, false, false);
const BIG: Lossy = lossy(false, true, false);
const OVF: Lossy = lossy(false, false, true);

/// 期望的 `json.loads(bytes)` 结果：`(repr, 丢信息标记)` 或错误类别。
type Decoded = Result<(&'static str, Lossy), JsonError>;

/// `(形状, 层数, 结尾, json.loads 的结果：最大深度或错误, body.parse_json 的结果)`
type DepthCase = (
    Shape,
    usize,
    Tail,
    Result<usize, JsonError>,
    Result<(), JsonError>,
);

/// `json.loads(raw)`：成功给出 `(repr, 丢信息标记)`。
fn loads(raw: &[u8]) -> Result<(String, Lossy), JsonError> {
    let l = pyloads::loads_bytes(raw, Options::LOADS)?;
    let flags = lossy(l.lone_surrogate, l.big_int, l.float_overflow);
    Ok((l.value.py_repr(), flags))
}

/// `json.dumps(body.parse_json(raw), ensure_ascii=False, separators=(",", ":"))`。
fn wire(raw: &[u8]) -> Result<String, JsonError> {
    let p = json_body::parse_json(raw)?;
    assert!(!p.lossy.any(), "{}", raw.escape_ascii());
    Ok(String::from_utf8(pyjson::wire(&p.value)).unwrap())
}

fn utf16(s: &str, big_endian: bool) -> Vec<u8> {
    let unit = |u: u16| {
        if big_endian {
            u.to_be_bytes()
        } else {
            u.to_le_bytes()
        }
    };
    s.encode_utf16().flat_map(unit).collect()
}

fn utf32(s: &str, big_endian: bool) -> Vec<u8> {
    let unit = |c: char| {
        if big_endian {
            u32::from(c).to_be_bytes()
        } else {
            u32::from(c).to_le_bytes()
        }
    };
    s.chars().flat_map(unit).collect()
}

#[derive(Debug, Clone, Copy)]
enum Shape {
    Arr,
    Obj,
    ArrEmptyLeaf,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Tail {
    Closed,
    /// 闭合后多一个 " x"
    Junk,
    /// 不闭合
    Open,
}

/// 嵌套 n 层的文档：`[[[1]]]`、`{"a":{"a":1}}`、`[[[]]]`（最里面是空数组，它自己也算一层）。
fn nested(shape: Shape, n: usize, tail: Tail) -> Vec<u8> {
    let (head, leaf, close) = match shape {
        Shape::Arr => (b"[".repeat(n), &b"1"[..], b"]".repeat(n)),
        Shape::Obj => (b"{\"a\":".repeat(n), &b"1"[..], b"}".repeat(n)),
        Shape::ArrEmptyLeaf => (b"[".repeat(n - 1), &b"[]"[..], b"]".repeat(n - 1)),
    };
    let mut out = head;
    out.extend_from_slice(leaf);
    if tail != Tail::Open {
        out.extend_from_slice(&close);
        if tail == Tail::Junk {
            out.extend_from_slice(b" x");
        }
    }
    out
}

#[test]
fn loads_accepts_what_python_accepts() {
    // (输入, repr(json.loads(输入)))
    let table: &[(&[u8], &str)] = &[
        (b"null", "None"),
        (b"true", "True"),
        (b"false", "False"),
        (b" null ", "None"),
        (b"\t\n\r null \t\n\r", "None"),
        (b"0", "0"),
        (b"-0", "0"),
        (b"1", "1"),
        (b"-1", "-1"),
        (b"1.0", "1.0"),
        (b"-0.0", "-0.0"),
        (b"-0e0", "-0.0"),
        (b"0e0", "0.0"),
        (b"1e2", "100.0"),
        (b"1E2", "100.0"),
        (b"1e+2", "100.0"),
        (b"1e-2", "0.01"),
        (b"1.5E-3", "0.0015"),
        (b"0.1", "0.1"),
        (b"1.7976931348623157e308", "1.7976931348623157e+308"),
        (b"5e-324", "5e-324"),
        (b"2e-324", "0.0"),
        (b"1e-999", "0.0"),
        (b"18446744073709551615", "18446744073709551615"),
        (b"-9223372036854775808", "-9223372036854775808"),
        (b"9223372036854775807", "9223372036854775807"),
        (b"[]", "[]"),
        (b"{}", "{}"),
        (b"[[]]", "[[]]"),
        (b"[{}]", "[{}]"),
        (br#"{"a":[]}"#, "{'a': []}"),
        (b"[1,2,3]", "[1, 2, 3]"),
        (b" [ 1 , 2 ] ", "[1, 2]"),
        (
            br#"{"a": 1, "b": [true, null]}"#,
            "{'a': 1, 'b': [True, None]}",
        ),
        (b"[1.0, 1, -0, -0.0]", "[1.0, 1, 0, -0.0]"),
        (br#"{"":1}"#, "{'': 1}"),
        (br#"{"a b":"c d"}"#, "{'a b': 'c d'}"),
        (br#"["a","a"]"#, "['a', 'a']"),
        (br#"{"a":1,"b":2,"a":3}"#, "{'a': 3, 'b': 2}"),
        (br#"{"a":1,"a":2,"a":3}"#, "{'a': 3}"),
        (br#"{"b":1,"a":2,"b":3,"c":4}"#, "{'b': 3, 'a': 2, 'c': 4}"),
        (br#"{"a":{"x":1},"a":[2]}"#, "{'a': [2]}"),
        (br#""abc""#, "'abc'"),
        (br#""""#, "''"),
        (br#""\u00e9""#, "'é'"),
        (br#""\u0041""#, "'A'"),
        (br#""\n\t\r\b\f\/\\\"""#, "'\\n\\t\\r\\x08\\x0c/\\\\\"'"),
        (br#""\u0000""#, "'\\x00'"),
        (b"\"\x7f\"", "'\\x7f'"),
        (b"\"\xe7\x8c\xab\"", "'猫'"),
        (b"\"\xf0\x9f\xa6\x89\"", "'🦉'"),
        (br#""\u732b""#, "'猫'"),
        (br#""\ud83d\ude00""#, "'😀'"),
        (br#""\ud83e\udd89x""#, "'🦉x'"),
        (br#"{"\u732b":"\u00e9"}"#, "{'猫': 'é'}"),
        (b"NaN", "nan"),
        (b"Infinity", "inf"),
        (b"-Infinity", "-inf"),
        (b"[NaN, Infinity, -Infinity]", "[nan, inf, -inf]"),
        (br#"{"a":NaN}"#, "{'a': nan}"),
        (b"\xef\xbb\xbf[1]", "[1]"),
    ];
    for &(raw, want) in table {
        let (repr, lost) = loads(raw)
            .unwrap_or_else(|e| panic!("{}：Python 接受，Rust 报 {e:?}", raw.escape_ascii()));
        assert_eq!(repr, want, "{}", raw.escape_ascii());
        assert_eq!(lost, NONE, "{}", raw.escape_ascii());
        // json.loads(str)：没有 BOM 的合法 UTF-8 走 str 入口，结果相同
        if let Ok(s) = std::str::from_utf8(raw)
            && !s.starts_with('\u{feff}')
        {
            let l = pyloads::loads_str(s, Options::LOADS).unwrap();
            assert_eq!(l.value.py_repr(), want, "str 入口 {}", raw.escape_ascii());
        }
    }
}

#[test]
fn loads_flags_what_serde_json_cannot_hold() {
    // (输入, repr(json.loads(输入)), 标记)。孤立代理在 Python 里是真的代理码位，这里的 repr 是 U+FFFD；
    // 大整数的 repr 与 Python 一致（Obj 保留全部数位）
    let table: &[(&[u8], &str, Lossy)] = &[
        (
            b"123456789012345678901234567890",
            "123456789012345678901234567890",
            BIG,
        ),
        (b"1e999", "inf", OVF),
        (b"-1e999", "-inf", OVF),
        (b"[1e308, 1e309]", "[1e+308, inf]", OVF),
        (b"18446744073709551616", "18446744073709551616", BIG),
        (b"-9223372036854775809", "-9223372036854775809", BIG),
        (
            b"340282366920938463463374607431768211456",
            "340282366920938463463374607431768211456",
            BIG,
        ),
        (
            b"1000000000000000000000000000000000000000",
            "1000000000000000000000000000000000000000",
            BIG,
        ),
        (
            b"-77777777777777777777777777777777777777777777777777",
            "-77777777777777777777777777777777777777777777777777",
            BIG,
        ),
        (b"1.7976931348623159e308", "inf", OVF),
        (br#""\ud800""#, "'\u{fffd}'", LONE),
        (br#""\udc00""#, "'\u{fffd}'", LONE),
        (br#""\ud800\ud800""#, "'\u{fffd}\u{fffd}'", LONE),
        (br#""\ude00\ud83d""#, "'\u{fffd}\u{fffd}'", LONE),
        (br#""\ud800\u0041""#, "'\u{fffd}A'", LONE),
        (br#"["\ud800", "ok"]"#, "['\u{fffd}', 'ok']", LONE),
        (br#"{"\ud800": 1}"#, "{'\u{fffd}': 1}", LONE),
        (br#"{"k": "\udfff"}"#, "{'k': '\u{fffd}'}", LONE),
        (b"\"\xed\xa0\x80\"", "'\u{fffd}'", LONE),
        (b"\"a\xed\xbf\xbfb\"", "'a\u{fffd}b'", LONE),
    ];
    for &(raw, want, want_flags) in table {
        let (repr, got_flags) =
            loads(raw).unwrap_or_else(|e| panic!("{}：Rust 报 {e:?}", raw.escape_ascii()));
        assert_eq!(repr, want, "{}", raw.escape_ascii());
        assert_eq!(got_flags, want_flags, "{}", raw.escape_ascii());
        assert!(got_flags.any());
    }
    // 边界：u64::MAX 与 i64::MIN 还装得下，不打标记
    for raw in [
        "18446744073709551615",
        "-9223372036854775808",
        "[9223372036854775807]",
    ] {
        assert_eq!(loads(raw.as_bytes()).unwrap().1, NONE, "{raw}");
    }
}

#[test]
fn loads_rejects_syntax_errors() {
    // json.JSONDecodeError → Syntax：多余内容、前导零、`1.`、`.5`、`-`、尾逗号、单引号、字符串里的控制字符……
    #[rustfmt::skip]
    let table: &[&[u8]] = &[
        b"", b" ", b"\n", b"{} x", b"{}x", b"[] []", b"01", b"-01", b"00", b"[01]", b"1.", b"[1.]",
        b".5", b"-.5", b"-", b"+1", b"--1", b"1e", b"1e+", b"0x10", b"1_000", b"[1,]", b"[,1]",
        b"[,]", br#"{"a":1,}"#, b"{,}", br#"{"a"}"#, br#"{"a":}"#, br#"{"a" 1}"#, b"{1:2}",
        b"{'a':1}", b"['a']", b"[1 2]", b"[1;2]", b"tru", b"nul", b"nulL", b"True", b"NULL",
        b"nan", b"infinity", b"Infinit", b"-Infinit", b"-Inf", b"+Infinity", br#""abc"#, br#"""#,
        br#""\x""#, br#""\u12""#, br#""\u12g4""#, br#""\ud800\u12""#, b"\"a\nb\"", b"\"a\tb\"",
        b"\"\x01\"", b"\"\x1f\"", b"\"\r\"", b"\x0b1", b"\x0c1", b"\xc2\xa01", b"\xe3\x80\x801",
        b"[", b"]", b"{", b"}", b"[1", b"[1}", br#"{"a":1]"#, b"[1]]", br#"{"a":1}}"#, b"1 2",
        br#""a" "b""#, b"//c\n1", b"/*c*/1", br#"\u0031"#, b"\xef\xbb\xbf",
        b"\xef\xbb\xbf\xef\xbb\xbf1", b"NaN1", b"[NaN", br#"{"a":NaN,}"#,
    ];
    for raw in table {
        assert_eq!(
            loads(raw).unwrap_err(),
            JsonError::Syntax,
            "{}",
            raw.escape_ascii()
        );
        // 同一段文本走 str 入口，是同一类错误
        let got = pyloads::loads_str(std::str::from_utf8(raw).unwrap(), Options::LOADS);
        assert_eq!(
            got.err(),
            Some(JsonError::Syntax),
            "str 入口 {}",
            raw.escape_ascii()
        );
    }
}

#[test]
fn bytes_are_decoded_like_json_detect_encoding() {
    // json.loads(bytes)：按 BOM / 空字节的位置识别 UTF-8(-SIG) / UTF-16 / UTF-32，再以 surrogatepass 解码。
    // 同一段文本换成各种编码，结果相同
    for (text, want) in [
        ("1", "1"),
        (r#"{"a":"猫"}"#, "{'a': '猫'}"),
        (r#"["🦉"]"#, "['🦉']"),
    ] {
        let encoded: [(&str, Vec<u8>); 9] = [
            (
                "utf-8-sig",
                [&[0xef, 0xbb, 0xbf][..], text.as_bytes()].concat(),
            ),
            (
                "utf-16（小端 BOM）",
                [&[0xff, 0xfe][..], &utf16(text, false)].concat(),
            ),
            (
                "utf-16（大端 BOM）",
                [&[0xfe, 0xff][..], &utf16(text, true)].concat(),
            ),
            ("utf-16-le", utf16(text, false)),
            ("utf-16-be", utf16(text, true)),
            (
                "utf-32（小端 BOM）",
                [&[0xff, 0xfe, 0, 0][..], &utf32(text, false)].concat(),
            ),
            (
                "utf-32（大端 BOM）",
                [&[0, 0, 0xfe, 0xff][..], &utf32(text, true)].concat(),
            ),
            ("utf-32-le", utf32(text, false)),
            ("utf-32-be", utf32(text, true)),
        ];
        for (name, bytes) in encoded {
            assert_eq!(
                loads(&bytes),
                Ok((want.to_string(), NONE)),
                "{text} / {name}"
            );
        }
    }
    // 其余：孤立代理、空输入、只有 BOM、长度不对、码点超界、非法 UTF-8
    let table: &[(&[u8], Decoded)] = &[
        // utf-16-le 孤立代理
        (b"\"\x00\x00\xd8\"\x00", Ok(("'\u{fffd}'", LONE))),
        // utf-16-le 高代理+低代理(分开)
        (b"\"\x00=\xd8\x00\xde\"\x00", Ok(("'😀'", NONE))),
        // utf-32-le 代理码位
        (
            b"\"\x00\x00\x00\x00\xd8\x00\x00\"\x00\x00\x00",
            Ok(("'\u{fffd}'", LONE)),
        ),
        // NUL 开头的 2 字节按 UTF-16-BE
        (b"\x001", Ok(("1", NONE))),
        // utf-8-sig
        (b"\xef\xbb\xbf{\"a\":1}", Ok(("{'a': 1}", NONE))),
        // utf-16-le 空
        (b"", Err(JsonError::Syntax)),
        // utf-16 仅 BOM
        (b"\xff\xfe", Err(JsonError::Syntax)),
        // utf-32 仅 BOM
        (b"\xff\xfe\x00\x00", Err(JsonError::Syntax)),
        // utf-16 奇数长度
        (b"\xff\xfe{", Err(JsonError::Decode)),
        // utf-16-le 奇数长度
        (b"{\x00}", Err(JsonError::Syntax)),
        // utf-32-le 长度不是 4 的倍数
        (b"{\x00\x00\x00}\x00", Err(JsonError::Decode)),
        // utf-32-le 码点超界
        (
            b"\"\x00\x00\x00\x00\x00\x11\x00\"\x00\x00\x00",
            Err(JsonError::Decode),
        ),
        // 非法 UTF-8 单字节
        (b"\xff", Err(JsonError::Decode)),
        // 非法 UTF-8 续字节
        (b"\xc3(", Err(JsonError::Decode)),
        // 过长编码
        (b"\"\xc0\x80\"", Err(JsonError::Decode)),
        // 5 字节序列
        (b"\"\xf8\x88\x80\x80\x80\"", Err(JsonError::Decode)),
        // 码点超过 10FFFF
        (b"\"\xf4\x90\x80\x80\"", Err(JsonError::Decode)),
        // 截断的 3 字节
        (b"\"\xe7\x8c\"", Err(JsonError::Decode)),
        // 截断的代理编码
        (b"\"\xed\xa0\"", Err(JsonError::Decode)),
        // 代理编码 ED A0 80
        (b"\"\xed\xa0\x80\"", Ok(("'\u{fffd}'", LONE))),
        // 字符串外的非法字节
        (b"[1,\xff]", Err(JsonError::Decode)),
        // 仅 BOM 之后非法
        (b"\xef\xbb\xbf\xff", Err(JsonError::Decode)),
    ];
    for (raw, want) in table {
        let got = loads(raw);
        let got = got
            .as_ref()
            .map(|(repr, f)| (repr.as_str(), *f))
            .map_err(Clone::clone);
        assert_eq!(got, want.clone(), "{}", raw.escape_ascii());
    }
    // str 入口以 U+FEFF 开头直接报错（Python：Unexpected UTF-8 BOM），bytes 入口会剥掉 BOM
    let got = pyloads::loads_str("\u{feff}{}", Options::LOADS);
    assert_eq!(got.err(), Some(JsonError::Syntax));
    assert!(pyloads::loads_bytes(b"\xef\xbb\xbf{}", Options::LOADS).is_ok());
}

#[test]
fn strict_mode_rejects_nan_and_infinity() {
    // body.parse_json 用 parse_constant 拒绝 NaN / Infinity / -Infinity，抛 ValueError("不支持 X")；
    // 常量先于它后面的语法错误被发现，所以 `[1, NaN, }` 报的是常量
    #[rustfmt::skip]
    let table: &[(&[u8], JsonError)] = &[
        (b"NaN", JsonError::Constant("NaN")), (b"[NaN]", JsonError::Constant("NaN")),
        (br#"{"a":NaN}"#, JsonError::Constant("NaN")), (b"[1, NaN, }", JsonError::Constant("NaN")),
        (b"Infinity", JsonError::Constant("Infinity")),
        (b"[Infinity]", JsonError::Constant("Infinity")),
        (b"-Infinity", JsonError::Constant("-Infinity")),
        (b"[-Infinity]", JsonError::Constant("-Infinity")),
        (br#"{"a": -Infinity, "b": }"#, JsonError::Constant("-Infinity")),
        (b"NaN1", JsonError::Constant("NaN")), (b"[1, }", JsonError::Syntax),
        (b"-Inf", JsonError::Syntax), (b"nan", JsonError::Syntax),
        (b"[NaN", JsonError::Constant("NaN")), (b"[1,NaN,Infinity]", JsonError::Constant("NaN")),
    ];
    for (raw, want) in table {
        let got = json_body::parse_json(raw).unwrap_err();
        assert_eq!(got, *want, "{}", raw.escape_ascii());
    }
    for (raw, msg) in [
        ("NaN", "不支持 NaN"),
        ("Infinity", "不支持 Infinity"),
        ("-Infinity", "不支持 -Infinity"),
    ] {
        let e = json_body::parse_json(raw.as_bytes()).unwrap_err();
        assert_eq!(e.to_string(), msg);
        assert_eq!(e.py_type(), "ValueError");
    }
}

#[test]
fn parse_json_keeps_python_values_and_key_order() {
    // (输入, json.dumps(json.loads(输入), ensure_ascii=False, separators=(",", ":")))
    let table: &[(&[u8], &str)] = &[
        (br#"{"a":1,"b":2,"a":3}"#, "{\"a\":3,\"b\":2}"),
        (br#"{"b":1,"a":2,"b":3,"c":4}"#, "{\"b\":3,\"a\":2,\"c\":4}"),
        (
            br#"{"z":1,"y":{"b":2,"a":1},"x":[3,2,1]}"#,
            "{\"z\":1,\"y\":{\"b\":2,\"a\":1},\"x\":[3,2,1]}",
        ),
        (
            b"[1, 1.0, -0, -0.0, 1e2, 1E-2]",
            "[1,1.0,0,-0.0,100.0,0.01]",
        ),
        (b"-0", "0"),
        (b"-0.0", "-0.0"),
        (b"1", "1"),
        (b"1.0", "1.0"),
        (
            b"[1e16, 1e-5, 0.1, 5e-324, 1.7976931348623157e308, 123456789.125]",
            "[1e+16,1e-05,0.1,5e-324,1.7976931348623157e+308,123456789.125]",
        ),
        (
            b"[9223372036854775807, 18446744073709551615, -9223372036854775808]",
            "[9223372036854775807,18446744073709551615,-9223372036854775808]",
        ),
        (b"\"\xe7\x8c\xab\\u00e9\\n\"", "\"猫é\\n\""),
        (
            br#"{"k":"\u0000\u001f\u007f\u2028\ud83e\udd89"}"#,
            "{\"k\":\"\\u0000\\u001f\u{7f}\u{2028}🦉\"}",
        ),
        (br#"{"a":{"b":{"c":[]}}}"#, "{\"a\":{\"b\":{\"c\":[]}}}"),
        (b" \t\n{ \"a\" : [ 1 , 2 ] } \r\n", "{\"a\":[1,2]}"),
        (
            b"[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]",
            "[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]",
        ),
        (br#"{"":{"":""}}"#, "{\"\":{\"\":\"\"}}"),
        (br#"["\"", "\\", "\/"]"#, "[\"\\\"\",\"\\\\\",\"/\"]"),
        (b"[true,false,null]", "[true,false,null]"),
        (b"\xef\xbb\xbf{\"a\":1}", "{\"a\":1}"),
        (br#"{"NaN":1}"#, r#"{"NaN":1}"#),
        (br#""Infinity""#, r#""Infinity""#),
    ];
    for &(raw, want) in table {
        assert_eq!(wire(raw).as_deref(), Ok(want), "{}", raw.escape_ascii());
    }
    // 32 层通过
    for shape in [Shape::Arr, Shape::Obj] {
        assert!(json_body::parse_json(&nested(shape, MAX_JSON_DEPTH, Tail::Closed)).is_ok());
    }
    // 重复键：值取最后一个、位置取第一次；键多到 PyDict 改用索引（≥ 16 个）时也一样
    let mut text = String::from("{");
    for i in 0..40 {
        text += &format!("\"k{i}\":{i},");
    }
    text += "\"k3\":99}";
    let p = json_body::parse_json(text.as_bytes()).unwrap();
    let obj = p.value.as_object().unwrap();
    assert_eq!(obj.len(), 40);
    assert!(obj.keys().enumerate().all(|(i, k)| *k == format!("k{i}")));
    assert_eq!(obj["k3"], 99);
    assert_eq!(obj["k39"], 39);
    let l = pyloads::loads_bytes(text.as_bytes(), Options::LOADS).unwrap();
    assert!(
        l.value
            .py_repr()
            .starts_with("{'k0': 0, 'k1': 1, 'k2': 2, 'k3': 99, 'k4': 4, ")
    );
}

#[test]
fn json_numbers_keep_the_int_float_distinction() {
    // 1 是整数、1.0 是浮点；-0 是整数 0、-0.0 是浮点负零；1e2 是浮点
    let raw = b"[1, 1.0, -0, -0.0, 1e2, 0e0, 18446744073709551615, -9223372036854775808]";
    let p = json_body::parse_json(raw).unwrap();
    let a = p.value.as_array().unwrap();
    assert!(a[0].is_i64() && a[0].as_i64() == Some(1));
    assert!(a[1].is_f64() && a[1].as_f64() == Some(1.0));
    assert!(a[2].is_i64() && a[2].as_i64() == Some(0));
    assert!(a[3].is_f64() && a[3].as_f64().unwrap().is_sign_negative());
    assert!(a[4].is_f64() && a[4].as_f64() == Some(100.0));
    assert!(a[5].is_f64() && a[5].as_f64() == Some(0.0));
    assert!(a[6].is_u64() && a[6].as_u64() == Some(u64::MAX));
    assert!(a[7].is_i64() && a[7].as_i64() == Some(i64::MIN));
    assert_eq!(p.lossy, NONE);
}

#[test]
fn lossy_values_are_clamped_as_documented() {
    // 模块文档的约定（不是 Python 的结果）：孤立代理 → U+FFFD；超出 u64 / i64 的整数 → 最近的 f64
    // （Python 的 float(int)：2**64 → 1.8446744073709552e+19）；溢出浮点 → ±f64::MAX
    #[rustfmt::skip]
    let table: &[(&[u8], &str, Lossy)] = &[
        (br#"["\ud800"]"#, "[\"\u{fffd}\"]", LONE),
        (br#"{"\udc00":"\ud83d"}"#, "{\"\u{fffd}\":\"\u{fffd}\"}", LONE),
        (b"[\"\xf0\x9f\xa6\x89\"]", "[\"\u{1f989}\"]", NONE),
        (b"18446744073709551616", "1.8446744073709552e+19", BIG),
        (b"-9223372036854775809", "-9.223372036854776e+18", BIG),
        (b"1000000000000000000000000000000000000000", "1e+39", BIG),
        (b"-77777777777777777777777777777777777777777777777777", "-7.777777777777778e+49", BIG),
        (b"1e999", "1.7976931348623157e+308", OVF),
        (b"-1e999", "-1.7976931348623157e+308", OVF),
        (b"1.7976931348623159e308", "1.7976931348623157e+308", OVF),
        (b"[1e308, 1e309]", "[1e+308,1.7976931348623157e+308]", OVF),
        (b"1e-999", "0.0", NONE),
        (b"18446744073709551615", "18446744073709551615", NONE),
        (b"-9223372036854775808", "-9223372036854775808", NONE),
    ];
    for &(raw, want, want_flags) in table {
        let p =
            json_body::parse_json(raw).unwrap_or_else(|e| panic!("{}：{e:?}", raw.escape_ascii()));
        let got = String::from_utf8(pyjson::wire(&p.value)).unwrap();
        assert_eq!(got, want, "{}", raw.escape_ascii());
        assert_eq!(p.lossy, want_flags, "{}", raw.escape_ascii());
    }
    // 4300 位整数超出 f64 范围：夹到 f64::MAX / f64::MIN
    for (sign, want) in [
        ("", "1.7976931348623157e+308"),
        ("-", "-1.7976931348623157e+308"),
    ] {
        let raw = format!("{sign}{}", "9".repeat(MAX_INT_DIGITS));
        let p = json_body::parse_json(raw.as_bytes()).unwrap();
        assert_eq!(String::from_utf8(pyjson::wire(&p.value)).unwrap(), want);
        assert_eq!(p.lossy, BIG);
    }
}

#[test]
fn int_literal_digit_limit() {
    // 4300 位整数可以，4301 位 ValueError（sys.int_info.default_max_str_digits）；符号不算位数；
    // 带小数点或指数的是浮点，不受这个限制
    assert_eq!(MAX_INT_DIGITS, 4300);
    for sign in ["", "-"] {
        let ok = format!("{sign}{}", "9".repeat(MAX_INT_DIGITS));
        assert_eq!(loads(ok.as_bytes()), Ok((ok.clone(), BIG)));
        let too_long = format!("{sign}{}", "9".repeat(MAX_INT_DIGITS + 1));
        assert_eq!(loads(too_long.as_bytes()), Err(JsonError::IntTooLong));
        let nested = format!("[1, {too_long}]");
        assert_eq!(loads(nested.as_bytes()), Err(JsonError::IntTooLong));
    }
    let long_float = format!("1{}.0", "0".repeat(5000));
    assert_eq!(loads(long_float.as_bytes()), Ok(("inf".into(), OVF)));
    let long_exp = format!("{}e0", "9".repeat(5000));
    assert_eq!(loads(long_exp.as_bytes()), Ok(("inf".into(), OVF)));
}

#[test]
fn nesting_limits() {
    // (形状, 层数, 结尾, json.loads 的结果：最大深度或错误, body.parse_json 的结果)
    // json.loads 在 9998 层抛 RecursionError（C 扫描器的递归上限，CPython 3.12.14 实测）；
    // body.parse_json 另外拒绝超过 32 层的文档（ValueError("JSON 嵌套过深")）；
    // 语法错误先于深度检查被发现，所以多了 " x" 的 33 层文档报语法错误
    use JsonError::{Recursion, Syntax, TooDeep};
    use Shape::{Arr, ArrEmptyLeaf, Obj};
    use Tail::{Closed, Junk, Open};
    #[rustfmt::skip]
    let table: &[DepthCase] = &[
        (Arr, 1, Closed, Ok(1), Ok(())),
        (Arr, 2, Closed, Ok(2), Ok(())),
        (Arr, 31, Closed, Ok(31), Ok(())),
        (Arr, 32, Closed, Ok(32), Ok(())),
        (Arr, 33, Closed, Ok(33), Err(TooDeep)),
        (Arr, 34, Closed, Ok(34), Err(TooDeep)),
        (Arr, 100, Closed, Ok(100), Err(TooDeep)),
        (Arr, 5000, Closed, Ok(5000), Err(TooDeep)),
        (Arr, 9996, Closed, Ok(9996), Err(TooDeep)),
        (Arr, 9997, Closed, Ok(9997), Err(TooDeep)),
        (Arr, 9998, Closed, Err(Recursion), Err(Recursion)),
        (Arr, 9999, Closed, Err(Recursion), Err(Recursion)),
        (Arr, 100000, Closed, Err(Recursion), Err(Recursion)),
        (Obj, 1, Closed, Ok(1), Ok(())),
        (Obj, 32, Closed, Ok(32), Ok(())),
        (Obj, 33, Closed, Ok(33), Err(TooDeep)),
        (Obj, 9997, Closed, Ok(9997), Err(TooDeep)),
        (Obj, 9998, Closed, Err(Recursion), Err(Recursion)),
        (Obj, 100000, Closed, Err(Recursion), Err(Recursion)),
        // 最里面是空数组：它自己也算一层
        (ArrEmptyLeaf, 32, Closed, Ok(32), Ok(())),
        (ArrEmptyLeaf, 33, Closed, Ok(33), Err(TooDeep)),
        (ArrEmptyLeaf, 9997, Closed, Ok(9997), Err(TooDeep)),
        (ArrEmptyLeaf, 9998, Closed, Err(Recursion), Err(Recursion)),
        // 结尾多了 " x" / 没闭合：语法错误；但到了递归上限还是先报 RecursionError
        (Arr, 1, Junk, Err(Syntax), Err(Syntax)),
        (Arr, 33, Junk, Err(Syntax), Err(Syntax)),
        (Arr, 9997, Junk, Err(Syntax), Err(Syntax)),
        (Arr, 9998, Junk, Err(Recursion), Err(Recursion)),
        (Obj, 33, Junk, Err(Syntax), Err(Syntax)),
        (Obj, 9998, Junk, Err(Recursion), Err(Recursion)),
        (Arr, 1, Open, Err(Syntax), Err(Syntax)),
        (Arr, 33, Open, Err(Syntax), Err(Syntax)),
        (Arr, 20000, Open, Err(Recursion), Err(Recursion)),
        (Obj, 33, Open, Err(Syntax), Err(Syntax)),
        (Obj, 20000, Open, Err(Recursion), Err(Recursion)),
    ];
    for &(shape, n, tail, ref want_loads, ref want_parse) in table {
        let raw = nested(shape, n, tail);
        let what = format!("{shape:?} {n} 层 {tail:?}");
        let got = pyloads::loads_bytes(&raw, Options::LOADS).map(|l| l.max_depth);
        assert_eq!(&got, want_loads, "json.loads {what}");
        let got = json_body::parse_json(&raw).map(|_| ());
        assert_eq!(&got, want_parse, "body.parse_json {what}");
    }
    assert_eq!(MAX_JSON_DEPTH, 32);
    assert_eq!(RECURSION_DEPTH, 9998);
}

#[test]
fn json_whitespace_is_only_four_characters() {
    // 空格 \t \n \r 之外的空白（\x0b \x0c、NBSP、全角空格……）都是语法错误
    for ok in [" ", "\t", "\n", "\r"] {
        assert!(loads(format!("{ok}1{ok}").as_bytes()).is_ok(), "{ok:?}");
    }
    for bad in [
        "\x0b", "\x0c", "\u{a0}", "\u{3000}", "\u{2028}", "\u{85}", "\u{200b}",
    ] {
        let err = Err(JsonError::Syntax);
        assert_eq!(loads(format!("{bad}1").as_bytes()), err, "{bad:?}");
        assert_eq!(loads(format!("[1,{bad}2]").as_bytes()), err, "{bad:?}");
    }
}

#[test]
fn request_body_is_accepted_or_rejected_with_a_400_message() {
    // parse_json_body：解析失败（语法、编码、NaN、过深）→ 「请求体不是合法 JSON」；
    // 能解析但顶层不是对象 → 「请求体必须是 JSON 对象」；成功给出对象（键顺序同请求）
    let table: &[(&[u8], Result<&str, BodyJsonError>)] = &[
        (b"{}", Ok("{}")),
        (br#"{"a":1}"#, Ok("{\"a\":1}")),
        (b"{\"model\":\"x\",\"prompt\":\"\xe7\x8c\xab\"}", Ok("{\"model\":\"x\",\"prompt\":\"猫\"}")),
        (br#" {"a":1} "#, Ok("{\"a\":1}")),
        (b"\xef\xbb\xbf{\"a\":1}", Ok("{\"a\":1}")),
        (b"\xff\xfe{\x00\"\x00a\x00\"\x00:\x00\"\x00+s\"\x00}\x00", Ok("{\"a\":\"猫\"}")),
        (b"{\x00\x00\x00\"\x00\x00\x00a\x00\x00\x00\"\x00\x00\x00:\x00\x00\x001\x00\x00\x00}\x00\x00\x00", Ok("{\"a\":1}")),
        (b"[]", Err(BodyJsonError::NotObject)),
        (b"[1]", Err(BodyJsonError::NotObject)),
        (b"[{}]", Err(BodyJsonError::NotObject)),
        (b"1", Err(BodyJsonError::NotObject)),
        (br#""x""#, Err(BodyJsonError::NotObject)),
        (b"null", Err(BodyJsonError::NotObject)),
        (b"true", Err(BodyJsonError::NotObject)),
        (b"1.5", Err(BodyJsonError::NotObject)),
        (b"-0", Err(BodyJsonError::NotObject)),
        (b"", Err(BodyJsonError::Invalid)),
        (b" ", Err(BodyJsonError::Invalid)),
        (b"{", Err(BodyJsonError::Invalid)),
        (br#"{"a":1"#, Err(BodyJsonError::Invalid)),
        (br#"{"a":1}x"#, Err(BodyJsonError::Invalid)),
        (br#"{"a":1,}"#, Err(BodyJsonError::Invalid)),
        (br#"{"a":NaN}"#, Err(BodyJsonError::Invalid)),
        (br#"{"a":Infinity}"#, Err(BodyJsonError::Invalid)),
        (b"NaN", Err(BodyJsonError::Invalid)),
        (b"[NaN]", Err(BodyJsonError::Invalid)),
        (b"-Infinity", Err(BodyJsonError::Invalid)),
        (b"\xff", Err(BodyJsonError::Invalid)),
        (b"{\"a\":\"\xc3(\"}", Err(BodyJsonError::Invalid)),
        (br#"{"a":1}}"#, Err(BodyJsonError::Invalid)),
        (b"[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]", Err(BodyJsonError::Invalid)),
        (br#"{"a":"99999"}"#, Ok("{\"a\":\"99999\"}")),
    ];
    for &(raw, want) in table {
        let got = json_body::parse_json_body(raw).map(|(m, lost)| {
            assert!(!lost.any());
            String::from_utf8(pyjson::wire(&serde_json::Value::Object(m))).unwrap()
        });
        assert_eq!(got, want.map(str::to_string), "{}", raw.escape_ascii());
    }
    // 过深的文档是「不合法」，不是「不是对象」
    for (shape, n) in [
        (Shape::Obj, 33),
        (Shape::Arr, 33),
        (Shape::Arr, 9998),
        (Shape::Obj, 100000),
    ] {
        let raw = nested(shape, n, Tail::Closed);
        let got = json_body::parse_json_body(&raw).unwrap_err();
        assert_eq!(got, BodyJsonError::Invalid, "{shape:?} {n} 层");
    }
    let raw = nested(Shape::Obj, MAX_JSON_DEPTH, Tail::Closed);
    assert!(json_body::parse_json_body(&raw).is_ok());
    // 丢信息的输入照样接受，标记交给调用方处理
    let raw = br#"{"a":"\ud800","b":1e999,"c":123456789012345678901234567890}"#;
    assert_eq!(
        json_body::parse_json_body(raw).unwrap().1,
        lossy(true, true, true)
    );
    // 文案、错误类型和常量
    assert_eq!(
        MSG_INVALID_JSON,
        "请求格式错误，未处理。请更新或更换客户端。开发者参考：请求体不是合法 JSON"
    );
    assert_eq!(
        MSG_NOT_OBJECT,
        "请求格式错误，未处理。请更新或更换客户端。开发者参考：请求体必须是 JSON 对象"
    );
    assert_eq!(BodyJsonError::Invalid.message(), MSG_INVALID_JSON);
    assert_eq!(BodyJsonError::NotObject.message(), MSG_NOT_OBJECT);
    assert_eq!(BodyJsonError::Invalid.to_string(), MSG_INVALID_JSON);
    assert_eq!(BodyJsonError::NotObject.to_string(), MSG_NOT_OBJECT);
    assert_eq!(THREAD_PARSE_BYTES, 256 * 1024);
}

#[test]
fn error_kinds_map_to_python_exception_types() {
    let table = [
        (JsonError::Syntax, "JSONDecodeError"),
        (JsonError::Constant("NaN"), "ValueError"),
        (JsonError::IntTooLong, "ValueError"),
        (JsonError::TooDeep, "ValueError"),
        (JsonError::Decode, "UnicodeDecodeError"),
        (JsonError::Recursion, "RecursionError"),
    ];
    for (e, want) in table {
        assert_eq!(e.py_type(), want, "{e:?}");
    }
    assert_eq!(JsonError::TooDeep.to_string(), "JSON 嵌套过深");
}
