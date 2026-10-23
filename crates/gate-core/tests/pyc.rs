//! `pyc`：Rust 默认行为与 CPython 不同的那几处（舍入、求和、浮点转字符串、字符串转数字、
//! 按码点切片、空白判定、整除）。
//!
//! 每张表的期望值都是 CPython 3.12.14（Unicode 15.0）里实际跑出来的结果，注释写了对应的 Python 表达式；
//! 不是拿 Rust 实现反推的。表格用 `#[rustfmt::skip]` 按主题排成几列，方便对着 Python 逐行核对。

use gate_core::pyc;

const NAN: f64 = f64::NAN;
const INF: f64 = f64::INFINITY;

/// 浮点按位比较：区分 +0.0 / -0.0；NaN 只要两边都是 NaN 就算相等。
fn same(a: f64, b: f64) -> bool {
    (a.is_nan() && b.is_nan()) || a.to_bits() == b.to_bits()
}

fn assert_same(got: f64, want: f64, what: &str) {
    assert!(same(got, want), "{what}：得到 {got:?}，Python 是 {want:?}");
}

#[test]
fn round0_is_bankers_rounding() {
    // round(x)
    #[rustfmt::skip]
    let table: &[(f64, i64)] = &[
        (0.5, 0), (1.5, 2), (2.5, 2), (3.5, 4), (-0.5, 0), (-1.5, -2), (-2.5, -2),
        (0.49999999999999994, 0), (0.5000000000000001, 1),
        // 2^51 到 2^53 之间：半整数仍能表示，仍然取偶
        (2251799813685248.5, 2251799813685248), (2251799813685249.5, 2251799813685250),
        (4503599627370494.5, 4503599627370494), (4503599627370495.5, 4503599627370496),
        (-4503599627370495.5, -4503599627370496), (1000000000000000.5, 1000000000000000),
        (9007199254740992.0, 9007199254740992), (9007199254740994.0, 9007199254740994),
    ];
    for &(x, want) in table {
        assert_eq!(pyc::round0(x), want, "round({x:?})");
    }
}

#[test]
fn round_ndigits_rounds_the_exact_binary_value() {
    // round(x, n)：按 x 的精确二进制值做 ties-even，所以 2.675 → 2.67、1.005 → 1.0
    #[rustfmt::skip]
    let table: &[(f64, usize, f64)] = &[
        (2.675, 2, 2.67), (0.125, 2, 0.12), (0.375, 2, 0.38), (1.005, 2, 1.0), (0.285, 2, 0.28),
        (8.345, 2, 8.35), (0.045, 2, 0.04), (1.15, 1, 1.1), (-2.675, 2, -2.67), (2.675, 3, 2.675),
        (1234.5678, 2, 1234.57), (123456789.98765433, 4, 123456789.9877),
        (0.30000000000000004, 15, 0.3), (0.30000000000000004, 16, 0.3),
        (0.30000000000000004, 17, 0.30000000000000004),
        (7.0, 5, 7.0), (2.5, 1, 2.5), (1e22, 1, 1e22), (1e22, 0, 1e22), (1e300, 2, 1e300),
        // n = 0 时结果仍是浮点：round(2.5, 0) == 2.0
        (2.5, 0, 2.0), (1.5, 0, 2.0), (5.5, 0, 6.0), (-1.5, 0, -2.0),
        // 舍入到 0 保留符号：round(-0.001, 2) == -0.0
        (0.5, 0, 0.0), (-0.5, 0, -0.0), (-0.001, 2, -0.0), (-1e-10, 5, -0.0), (1e-10, 5, 0.0),
        (5e-324, 3, 0.0), (-0.0, 3, -0.0),
    ];
    for &(x, n, want) in table {
        assert_same(pyc::round(x, n), want, &format!("round({x:?}, {n})"));
    }
    // 非有限值原样返回；n 很大（> 300）时也原样返回：round(0.1, 400) == 0.1
    assert_same(pyc::round(NAN, 2), NAN, "round(nan, 2)");
    assert_same(pyc::round(INF, 2), INF, "round(inf, 2)");
    assert_same(pyc::round(-INF, 2), -INF, "round(-inf, 2)");
    assert_same(pyc::round(0.1, 400), 0.1, "round(0.1, 400)");
}

#[test]
fn sum_uses_neumaier_compensation() {
    // sum(xs)；xs 全是 float（Python 的 int 项不做补偿，这里的函数只收 f64）
    let table: Vec<(Vec<f64>, f64)> = vec![
        (vec![0.1; 10], 1.0),
        (vec![0.1; 3], 0.30000000000000004),
        (vec![0.1, 0.2, 0.3], 0.6),
        (vec![1e100, 1.0, -1e100], 1.0),
        (vec![3.0, 1e100, -1e100], 3.0),
        (vec![1e16, 1.0, -1e16], 1.0),
        (vec![1e-16, 1.0, -1.0], 1e-16),
        (vec![1.5], 1.5),
        (vec![0.0, -0.0], 0.0),
        (vec![-0.0, -0.0], 0.0),
        // sum([-0.0]) 在 3.12 里是 0.0：累加从整数 0 开始
        (vec![-0.0], 0.0),
        (vec![INF, -INF], NAN),
        (vec![1.0, INF], INF),
        (vec![-INF, 1.0], -INF),
        (vec![NAN, 1.0], NAN),
        (vec![1e308, 1e308], INF),
        (vec![1e308, 1e308, -1e308], INF),
        (vec![1e308, 1e308, -1e308, 1.0], INF),
    ];
    for (xs, want) in table {
        assert_same(pyc::sum(xs.clone()), want, &format!("sum({xs:?})"));
    }
    // sum([]) 在 Python 里是整数 0（与 0.0 相等）；这里是 +0.0
    assert_same(pyc::sum(Vec::<f64>::new()), 0.0, "sum([])");
    // 防止这张表变成空话：朴素累加在这两组上得到不同的数
    assert_ne!([0.1; 10].iter().sum::<f64>(), 1.0);
    assert_ne!([1e100, 1.0, -1e100].iter().sum::<f64>(), 1.0);
}

#[test]
fn float_repr_matches_python_repr() {
    // repr(x)
    #[rustfmt::skip]
    let table: &[(f64, &str)] = &[
        // 指数 ≥ 16 用科学计数法，指数至少两位并带符号
        (1e15, "1000000000000000.0"), (9999999999999998.0, "9999999999999998.0"), (1e16, "1e+16"),
        (12345678901234567.0, "1.2345678901234568e+16"), (123456789012345680.0, "1.2345678901234568e+17"),
        (1e17, "1e+17"), (1.23e20, "1.23e+20"), (1e22, "1e+22"), (1e100, "1e+100"), (1.5e300, "1.5e+300"),
        // 小于 1e-4 用科学计数法
        (1e-4, "0.0001"), (0.0001234, "0.0001234"), (1e-5, "1e-05"), (0.00001234, "1.234e-05"),
        (2.5e-5, "2.5e-05"), (1.5e-7, "1.5e-07"), (-1e-7, "-1e-07"), (1e-100, "1e-100"),
        // 整数值补 .0，负零保留符号，最短往返表示
        (0.0, "0.0"), (-0.0, "-0.0"), (1.0, "1.0"), (100.0, "100.0"), (123456.0, "123456.0"),
        (-1.5, "-1.5"), (-2.5, "-2.5"), (0.1, "0.1"), (100.5, "100.5"), (99999999999999.9, "99999999999999.9"), (4.35, "4.35"),
        (0.1 + 0.2, "0.30000000000000004"), (1.0 / 3.0, "0.3333333333333333"),
        (5e-324, "5e-324"), (2.2250738585072014e-308, "2.2250738585072014e-308"),
        (f64::MAX, "1.7976931348623157e+308"), (f64::MIN, "-1.7976931348623157e+308"),
        (NAN, "nan"), (-NAN, "nan"), (INF, "inf"), (-INF, "-inf"),
    ];
    for &(x, want) in table {
        assert_eq!(pyc::float_repr(x), want, "repr({x:?})");
    }
}

#[test]
fn parse_int_matches_python_int() {
    // int(s)；None = Python 抛 ValueError
    #[rustfmt::skip]
    let table: &[(&str, Option<i64>)] = &[
        // 首尾空白、正负号、前导零
        (" 12 ", Some(12)), (" 1_000 ", Some(1000)), ("\t\n12\x0b\x0c\r", Some(12)), ("\u{a0}12\u{a0}", Some(12)),
        ("\u{3000} 5 \u{3000}", Some(5)), ("\u{85} 7 \u{85}", Some(7)), ("\u{2028} 7", Some(7)),
        ("+1", Some(1)), ("-0", Some(0)), ("+0", Some(0)), ("-00", Some(0)), ("007", Some(7)),
        ("+", None), ("-", None), ("+-1", None), ("--1", None), ("+ 1", None), ("- 1", None), ("", None), ("   ", None),
        // 零宽空格、BOM、蒙古文元音分隔符、NUL 都不是空白
        ("\u{200b}5", None), ("5\u{200b}", None), ("\u{feff}1", None), ("1\u{feff}", None),
        ("\u{180e}1", None), ("1\u{2060}", None), ("1\u{0}1", None),
        // PEP 515 下划线：只能夹在两个数字之间
        ("1_000", Some(1000)), ("1_0_0", Some(100)), ("0_0", Some(0)),
        ("1__0", None), ("_1", None), ("1_", None), ("0_", None), ("+_1", None), ("-_1", None), ("1_-1", None),
        // 不是整数的写法
        ("1.0", None), ("12.0", None), ("1e3", None), ("0x10", None), ("1,000", None), ("1 000", None), ("1 2", None),
        // 任何 Unicode 十进制数字（Nd）都行（逐块的完整检查在下面那个测试里），全角正负号不行
        ("１２", Some(12)), ("-１", Some(-1)), ("٣", Some(3)), ("٣_٣", Some(33)), ("१२३", Some(123)),
        ("1١", Some(11)), ("𝟙𝟚𝟛", Some(123)),
        ("\u{11f50}\u{11f51}", Some(1)), // Unicode 15.0 新增的 Kawi 数字
        ("\u{10d40}", None),             // Unicode 16 才有的 Garay 数字：3.12 不认
        ("٣a", None), ("a٣", None), ("١٫٥", None), ("－1", None),
        // 范围：Python 是任意精度，这里是 i64，装不下一律 None（调用方都有默认值）
        ("9223372036854775807", Some(i64::MAX)), ("-9223372036854775807", Some(-i64::MAX)),
        ("9223372036854775808", None), ("-9223372036854775809", None), ("99999999999999999999", None),
        ("９９９９９９９９９９９９９９９９９９９９", None),
    ];
    for &(s, want) in table {
        assert_eq!(pyc::parse_int(s), want, "int({s:?})");
    }
    // 下面两类输入 Python 与 pyc::parse_int 的结果不同，故意不放进表里（不固定错误的一方）：
    // - "12\x1c"：Python 的 int() 只认 ASCII 空白，不把 \x1c–\x1f 当空白（str.strip() 才当）；
    // - "-9223372036854775808"：Python 是 -2^63，这里先解析绝对值再取负，2^63 装不进 i64，得 None。
}

#[test]
fn every_unicode_decimal_digit_is_accepted_and_nothing_else() {
    // Unicode 15.0 里全部 68 个十进制数字块的「0」（unicodedata 实测）
    #[rustfmt::skip]
    let zeros: [u32; 68] = [
        0x30, 0x660, 0x6f0, 0x7c0, 0x966, 0x9e6, 0xa66, 0xae6, 0xb66, 0xbe6, 0xc66, 0xce6, 0xd66, 0xde6,
        0xe50, 0xed0, 0xf20, 0x1040, 0x1090, 0x17e0, 0x1810, 0x1946, 0x19d0, 0x1a80, 0x1a90, 0x1b50,
        0x1bb0, 0x1c40, 0x1c50, 0xa620, 0xa8d0, 0xa900, 0xa9d0, 0xa9f0, 0xaa50, 0xabf0, 0xff10,
        0x104a0, 0x10d30, 0x11066, 0x110f0, 0x11136, 0x111d0, 0x112f0, 0x11450, 0x114d0, 0x11650,
        0x116c0, 0x11730, 0x118e0, 0x11950, 0x11c50, 0x11d50, 0x11da0, 0x11f50, 0x16a60, 0x16ac0,
        0x16b50, 0x1d7ce, 0x1d7d8, 0x1d7e2, 0x1d7ec, 0x1d7f6, 0x1e140, 0x1e2f0, 0x1e4f0, 0x1e950, 0x1fbf0,
    ];
    // 每个块的 1..9 拼起来就是 123456789，浮点同理
    for z in zeros {
        let digits: String = (1..10).map(|i| char::from_u32(z + i).unwrap()).collect();
        assert_eq!(pyc::parse_int(&digits), Some(123456789), "U+{z:04X} 块");
        let float = pyc::parse_float(&format!("{digits}.5")).unwrap();
        assert_same(float, 123456789.5, &format!("U+{z:04X} 块的浮点"));
    }
}

#[test]
fn parse_float_matches_python_float() {
    // float(s)；None = Python 抛 ValueError
    #[rustfmt::skip]
    let table: &[(&str, Option<f64>)] = &[
        // inf / nan：不区分大小写，可带正负号
        ("inf", Some(INF)), ("-Infinity", Some(-INF)), ("+Infinity", Some(INF)), ("INF", Some(INF)),
        ("iNfInItY", Some(INF)), (" -inf ", Some(-INF)), ("nan", Some(NAN)), ("+nan", Some(NAN)),
        ("-NaN", Some(NAN)), ("nan ", Some(NAN)),
        ("infinit", None), ("infinityx", None), ("in f", None), ("- inf", None), ("in_f", None),
        ("nan_", None), ("nan1", None),
        // 溢出得 inf，下溢得 0，不报错
        ("1e400", Some(INF)), ("-1e400", Some(-INF)), ("1.7976931348623159e308", Some(INF)),
        ("1e308", Some(1e308)), ("1e-400", Some(0.0)), ("4.9e-324", Some(5e-324)), ("2e-324", Some(0.0)),
        // 常见写法
        ("1e5", Some(100000.0)), ("1E-2", Some(0.01)), ("1E+05", Some(100000.0)), ("1e0001", Some(10.0)),
        ("+.5e1", Some(5.0)), (" .5 ", Some(0.5)), ("5.", Some(5.0)), ("1.e5", Some(100000.0)),
        ("00.5", Some(0.5)), ("0.1", Some(0.1)), ("0e0", Some(0.0)), ("-0", Some(-0.0)), ("-0.0", Some(-0.0)),
        ("-0e0", Some(-0.0)), ("\u{3000} 1.5 \u{3000}", Some(1.5)),
        // 下划线
        ("1_0.5", Some(10.5)), ("1_000.000_1", Some(1000.0001)), ("1_2_3", Some(123.0)), ("0_0", Some(0.0)),
        ("1e1_0", Some(1e10)), ("1e5_0", Some(1e50)), ("1_000e1_0", Some(1e13)),
        ("1_", None), ("1__0", None), ("1__2", None), ("_1.5", None), ("1_.5", None), ("1._5", None),
        ("1.5_", None), ("1_e5", None), ("1e_5", None), ("1.5e_1", None), ("1.5e1_", None),
        // 不是浮点数的写法
        ("", None), (".", None), ("e5", None), ("1e", None), ("5e", None), (".5e", None), (".e1", None),
        ("1e+", None), ("5e+-3", None), ("--1", None), ("0x10", None), ("1,5", None), ("1.5.5", None),
        ("1 2", None), ("\u{200b}1", None), ("−1", None), // 最后一个是 U+2212 数学减号
        // Unicode 数字
        ("１.５", Some(1.5)), ("٢.٥", Some(2.5)), ("٣e٢", Some(300.0)), ("1١.5", Some(11.5)),
        ("\u{11f51}.\u{11f55}", Some(1.5)),
    ];
    for &(s, want) in table {
        match (pyc::parse_float(s), want) {
            (Some(got), Some(want)) => assert_same(got, want, &format!("float({s:?})")),
            (got, want) => assert_eq!(got.is_some(), want.is_some(), "float({s:?})：{got:?}"),
        }
    }
    // 很长的输入：float("9" * 400) == inf，float("0." + "0" * 400 + "1") == 0.0
    assert_same(pyc::parse_float(&"9".repeat(400)).unwrap(), INF, "9*400");
    let tiny = format!("0.{}1", "0".repeat(400));
    assert_same(pyc::parse_float(&tiny).unwrap(), 0.0, "0.0…01");
}

#[test]
fn int_of_truncates_and_parse_int_via_float_composes() {
    // int(x)：向零截断；nan 抛 ValueError、inf 抛 OverflowError，这里都是 None
    #[rustfmt::skip]
    let table: &[(f64, Option<i64>)] = &[
        (15.9, Some(15)), (-15.9, Some(-15)), (-0.5, Some(0)), (0.0, Some(0)), (-0.0, Some(0)),
        (1000000000000000.5, Some(1000000000000000)), (9007199254740992.0, Some(9007199254740992)),
        (-1e18, Some(-1_000_000_000_000_000_000)), (NAN, None), (INF, None), (-INF, None),
    ];
    for &(x, want) in table {
        assert_eq!(pyc::int_of(x), want, "int({x:?})");
    }
    // int(float(s))
    #[rustfmt::skip]
    let table: &[(&str, Option<i64>)] = &[
        ("15.9", Some(15)), ("-15.9", Some(-15)), ("-0.9", Some(0)), ("1e3", Some(1000)),
        ("1_0.9", Some(10)), ("  42  ", Some(42)), ("٣.٩", Some(3)),
        ("inf", None), ("-inf", None), ("nan", None), ("abc", None), ("", None), ("0x10", None),
    ];
    for &(s, want) in table {
        assert_eq!(pyc::parse_int_via_float(s), want, "int(float({s:?}))");
    }
}

#[test]
fn strip_matches_str_strip() {
    // s.strip()
    #[rustfmt::skip]
    let table: &[(&str, &str)] = &[
        ("\x1c\x1d\x1e\x1fab\x1f", "ab"), ("\u{3000}x\u{3000}", "x"), ("\u{85}x\u{85}", "x"),
        ("\u{a0}x\u{a0}", "x"), ("\u{2028}x\u{2029}", "x"), ("\u{202f}x\u{205f}", "x"),
        ("\u{1680}x\u{2000}\u{200a}", "x"), (" \t\n\r\x0b\x0cx \t\n\r\x0b\x0c", "x"),
        (" a b ", "a b"), ("a b", "a b"), ("   ", ""), ("", ""),
        // 零宽空格、BOM、蒙古文元音分隔符、字连接符、NUL 都不是空白
        ("\u{200b}x\u{200b}", "\u{200b}x\u{200b}"), ("\u{feff}x\u{feff}", "\u{feff}x\u{feff}"),
        ("\u{180e}x", "\u{180e}x"), ("\u{2060}x", "\u{2060}x"), ("\0x\0", "\0x\0"),
    ];
    for &(s, want) in table {
        assert_eq!(pyc::strip(s), want, "{s:?}.strip()");
    }
}

#[test]
fn slicing_and_len_count_code_points() {
    // s[:n]；👨\u{200d}👩\u{200d}👧 是 3 个表情加 2 个 ZWJ，é 可以写成 e + 组合重音
    #[rustfmt::skip]
    let take: &[(&str, usize, &str)] = &[
        ("a\u{1f989}b", 2, "a\u{1f989}"), ("猫头鹰abc", 2, "猫头"), ("ab", 5, "ab"), ("ab", 0, ""), ("", 3, ""),
        ("e\u{301}x", 1, "e"), // 组合字符被切开，和 Python 一样
        ("e\u{301}x", 2, "e\u{301}"),
        ("\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}x", 3, "\u{1f468}\u{200d}\u{1f469}"),
    ];
    for &(s, n, want) in take {
        assert_eq!(pyc::take_chars(s, n), want, "{s:?}[:{n}]");
    }
    // s[-n:]（n ≥ 1）
    #[rustfmt::skip]
    let last: &[(&str, usize, &str)] = &[
        ("a\u{1f989}b", 2, "\u{1f989}b"), ("猫头鹰abc", 2, "bc"), ("猫头鹰abc", 6, "猫头鹰abc"),
        ("猫头鹰abc", 7, "猫头鹰abc"), ("ab", 5, "ab"), ("", 3, ""),
        ("e\u{301}x", 2, "\u{301}x"), ("e\u{301}x", 1, "x"),
        ("\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}x", 2, "\u{1f467}x"),
    ];
    for &(s, n, want) in last {
        assert_eq!(pyc::last_chars(s, n), want, "{s:?}[-{n}:]");
    }
    // len(s)
    #[rustfmt::skip]
    let len: &[(&str, usize)] = &[
        ("a\u{1f989}b", 3), ("e\u{301}", 2), ("\u{e9}", 1), ("", 0), ("猫头鹰", 3), ("a\0b", 3),
        ("\u{10ffff}", 1), ("\u{1f989}", 1), ("\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}", 5),
    ];
    for &(s, want) in len {
        assert_eq!(pyc::len_chars(s), want, "len({s:?})");
    }
}

#[test]
fn float_floor_division() {
    // a // b，期望值取自 CPython（fmod 版算法）；注意 1.0 // 0.1 是 9.0，而 floor(1.0 / 0.1) 是 10.0。
    #[rustfmt::skip]
    let table: &[(f64, f64, f64)] = &[
        (7.5, 2.0, 3.0), (-7.5, 2.0, -4.0), (-0.5, 2.0, -1.0), (0.0, -1.0, -0.0), (7.0, 2.0, 3.0),
        (-1.0, 3.0, -1.0), (7200.0, 3600.0, 2.0), (3599.0, 3600.0, 0.0), (-1.0, 3600.0, -1.0),
        (3599.999, 3600.0, 0.0), (1.0, 0.1, 9.0),
    ];
    for &(a, b, want) in table {
        assert_same(pyc::floordiv_f(a, b), want, &format!("{a:?} // {b:?}"));
    }
}

#[test]
fn bool_str_is_capitalized() {
    // str(True) / str(False)
    assert_eq!(pyc::bool_str(true), "True");
    assert_eq!(pyc::bool_str(false), "False");
}
