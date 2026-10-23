//! `pyjson`：与 Python `json.dumps` 逐字节一致的序列化（写进库给 2.x 读的字符串、发给上游的请求体）。
//!
//! 期望值都是 CPython 3.12.14 的 `json.dumps` 实际输出，不是拿 Rust 实现反推的。
//! 两种参数组合：
//! - `json.dumps(x, ensure_ascii=False)`：默认分隔符（`pyjson::SPACED`，`dumps`）；
//! - `json.dumps(x, ensure_ascii=False, separators=(",", ":"))`：线上格式（`pyjson::WIRE`，`wire`）。

use gate_core::pyjson::{self, SPACED, Style, WIRE};
use serde_json::{Value, json};

fn dump(v: &Value, style: Style) -> String {
    String::from_utf8(pyjson::dumps_with(v, style)).unwrap()
}

#[test]
fn numbers_are_formatted_like_python_in_every_style() {
    // 数字的写法与参数组合无关：整数原样、浮点用 repr（1e+16、1e-05、整数值带 .0）
    let table: &[(Value, &str)] = &[
        (json!(0), "0"),
        (json!(-1), "-1"),
        (json!(i64::MAX), "9223372036854775807"),
        (json!(i64::MIN), "-9223372036854775808"),
        (json!(u64::MAX), "18446744073709551615"),
        (json!(9_007_199_254_740_993_i64), "9007199254740993"),
        (json!(1.0), "1.0"),
        (json!(-1.5), "-1.5"),
        (json!(0.0), "0.0"),
        (json!(-0.0), "-0.0"),
        (json!(0.1), "0.1"),
        (json!(100.0), "100.0"),
        (json!(123456789.12345679), "123456789.12345679"),
        (json!(0.30000000000000004), "0.30000000000000004"),
        (json!(1e15), "1000000000000000.0"),
        (json!(1e16), "1e+16"),
        (json!(9999999999999998.0), "9999999999999998.0"),
        (json!(9007199254740993.0), "9007199254740992.0"),
        (json!(1e22), "1e+22"),
        (json!(1e100), "1e+100"),
        (json!(0.0001), "0.0001"),
        (json!(1e-5), "1e-05"),
        (json!(2.5e-5), "2.5e-05"),
        (json!(1e-7), "1e-07"),
        (json!(5e-324), "5e-324"),
        (json!(f64::MAX), "1.7976931348623157e+308"),
        (json!(f64::MIN), "-1.7976931348623157e+308"),
    ];
    for (v, want) in table {
        for style in [SPACED, WIRE] {
            assert_eq!(dump(v, style), *want, "{v:?} / {style:?}");
        }
    }
    // 数组里同理，分隔符不影响数字本身
    assert_eq!(
        dump(&json!([1.0, 1e16, 1e-5, -0.0]), SPACED),
        "[1.0, 1e+16, 1e-05, -0.0]"
    );
    assert_eq!(
        dump(&json!([1.0, 1e16, 1e-5, -0.0]), WIRE),
        "[1.0,1e+16,1e-05,-0.0]"
    );
}

#[test]
fn f32_goes_through_f64() {
    // 先转 f64 再按 repr 写：Python 里等价于 repr(struct.unpack("f", struct.pack("f", x))[0])
    for (x, want) in [
        (0.1_f32, "0.10000000149011612"),
        (1.5, "1.5"),
        (16777217.0, "16777216.0"),
    ] {
        let bytes = pyjson::dumps_with(&x, WIRE);
        assert_eq!(String::from_utf8(bytes).unwrap(), want, "{x}");
    }
}

#[test]
fn separators_empty_containers_and_key_order() {
    // (值, 默认分隔符 ", " ": ", 紧凑 "," ":")；
    let table: &[(Value, &str, &str)] = &[
        (json!({}), "{}", "{}"),
        (json!([]), "[]", "[]"),
        (json!([[]]), "[[]]", "[[]]"),
        (json!([{}, []]), "[{}, []]", "[{},[]]"),
        (
            json!({"a": [], "b": {}}),
            "{\"a\": [], \"b\": {}}",
            "{\"a\":[],\"b\":{}}",
        ),
        (json!([1]), "[1]", "[1]"),
        (json!([1, 2, 3]), "[1, 2, 3]", "[1,2,3]"),
        (json!({"a": 1}), "{\"a\": 1}", "{\"a\":1}"),
        (
            json!({"a": 1, "b": 2}),
            "{\"a\": 1, \"b\": 2}",
            "{\"a\":1,\"b\":2}",
        ),
        (
            json!([true, false, null]),
            "[true, false, null]",
            "[true,false,null]",
        ),
        (
            json!({"b": 1, "a": 2, "c": 3}),
            "{\"b\": 1, \"a\": 2, \"c\": 3}",
            "{\"b\":1,\"a\":2,\"c\":3}",
        ),
        (
            json!({"z": {"b": 1, "a": 2}, "a": [3, 2, 1]}),
            "{\"z\": {\"b\": 1, \"a\": 2}, \"a\": [3, 2, 1]}",
            "{\"z\":{\"b\":1,\"a\":2},\"a\":[3,2,1]}",
        ),
        (
            json!({"x": [{"k": [1.0, {"n": null}]}, []], "": ""}),
            "{\"x\": [{\"k\": [1.0, {\"n\": null}]}, []], \"\": \"\"}",
            "{\"x\":[{\"k\":[1.0,{\"n\":null}]},[]],\"\":\"\"}",
        ),
        (
            json!([1, 2.5, "x", null, true, false, 1e16, 1e-5]),
            "[1, 2.5, \"x\", null, true, false, 1e+16, 1e-05]",
            "[1,2.5,\"x\",null,true,false,1e+16,1e-05]",
        ),
    ];
    for (v, default, compact) in table {
        assert_eq!(dump(v, SPACED), *default, "{v:?}");
        assert_eq!(dump(v, WIRE), *compact, "{v:?}");
    }
    // 键保持插入顺序，不按字典序：先 b 后 a
    let mut m = serde_json::Map::new();
    for k in ["b", "a", "c", "B", "_", "1"] {
        m.insert(k.into(), json!(0));
    }
    assert_eq!(
        pyjson::dumps(&Value::Object(m)),
        r#"{"b": 0, "a": 0, "c": 0, "B": 0, "_": 0, "1": 0}"#
    );
}

#[test]
fn string_escaping() {
    // (字符串, ensure_ascii=False 的输出)
    // 转义 " \ 和 < 0x20 的控制字符（\n \r \t \b \f 用短写法，其余 \u00xx 小写十六进制）；
    // \x7f、\u{2028}、非 BMP 字符都原样输出
    let table: &[(&str, &str)] = &[
        ("abc", "\"abc\""),
        ("", "\"\""),
        ("a b", "\"a b\""),
        ("~", "\"~\""),
        ("quote\"back\\slash/", "\"quote\\\"back\\\\slash/\""),
        ("a\nb", "\"a\\nb\""),
        ("a\tb", "\"a\\tb\""),
        ("a\rb", "\"a\\rb\""),
        ("a\u{8}b", "\"a\\bb\""),
        ("a\u{c}b", "\"a\\fb\""),
        ("\u{0}", "\"\\u0000\""),
        ("\u{1}", "\"\\u0001\""),
        ("\u{1b}", "\"\\u001b\""),
        ("\u{1f}", "\"\\u001f\""),
        ("\u{7f}", "\"\u{7f}\""),
        ("\u{80}", "\"\u{80}\""),
        ("\u{9f}", "\"\u{9f}\""),
        ("\u{a0}", "\"\u{a0}\""),
        ("é", "\"é\""),
        ("ÿ", "\"ÿ\""),
        ("Ā", "\"Ā\""),
        ("猫", "\"猫\""),
        ("\u{2028}", "\"\u{2028}\""),
        ("\u{2029}", "\"\u{2029}\""),
        ("\u{feff}", "\"\u{feff}\""),
        ("\u{200b}", "\"\u{200b}\""),
        ("\u{ffff}", "\"\u{ffff}\""),
        ("🦉", "\"🦉\""),
        ("𐀀", "\"𐀀\""),
        ("\u{10ffff}", "\"\u{10ffff}\""),
        ("猫\n🦉\"", "\"猫\\n🦉\\\"\""),
        ("aéb猫c🦉d", "\"aéb猫c🦉d\""),
    ];
    for (s, raw) in table {
        let v = Value::String((*s).into());
        for style in [SPACED, WIRE] {
            assert_eq!(dump(&v, style), *raw, "{s:?} / {style:?}");
        }
        // 对象的键走同一套转义
        let obj = json!({ *s: 0 });
        assert_eq!(dump(&obj, WIRE), format!("{{{raw}:0}}"), "键 {s:?}");
        assert_eq!(dump(&obj, SPACED), format!("{{{raw}: 0}}"), "键 {s:?}");
    }
}

#[test]
fn convenience_wrappers_match_the_styles() {
    let v = json!({"b": 1, "a": [1.0, "x", 1e-5, 1e16], "中": "猫\n\u{1F989}"});
    assert_eq!(
        String::from_utf8(pyjson::wire(&v)).unwrap(),
        "{\"b\":1,\"a\":[1.0,\"x\",1e-05,1e+16],\"中\":\"猫\\n🦉\"}"
    );
    assert_eq!(
        pyjson::dumps(&v),
        "{\"b\": 1, \"a\": [1.0, \"x\", 1e-05, 1e+16], \"中\": \"猫\\n🦉\"}"
    );
    assert_eq!(dump(&v, WIRE).as_bytes(), pyjson::wire(&v));
    assert_eq!(dump(&v, SPACED), pyjson::dumps(&v));
}

#[test]
fn loads_round_trips_and_rejects_invalid_text() {
    // json.loads：解析失败是 None（Python 抛 JSONDecodeError）；不认 NaN / Infinity 字面量（旧代码从不写出它们）
    let v =
        json!({"b": 1, "a": [1.0, "x", 1e-5, 1e16, -0.0], "c": {"d": null, "e": [true, false]}});
    for style in [SPACED, WIRE] {
        let text = dump(&v, style);
        let back = pyjson::loads(&text).unwrap();
        assert_eq!(dump(&back, style), text, "{style:?}");
    }
    assert_eq!(pyjson::loads("[1, 2.0]"), Some(json!([1, 2.0])));
    for bad in [
        "", "{", "[1,]", "{'a': 1}", "[1] x", "NaN", "Infinity", "01",
    ] {
        assert_eq!(pyjson::loads(bad), None, "{bad:?}");
    }
}
