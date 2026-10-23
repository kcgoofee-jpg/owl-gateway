//! `reasons`：拒绝原因码（落进 `usage_log.reason`，统计 / 限流 / 自适应上限都按它数）、`Reason` 的 code、
//! 给站长看的短说明，以及没有码时从 detail 里取短说明的回退规则。
//!
//! 期望值来自 Python 版 `reasons.py`（git 标签 `py-final`）在 CPython 3.12.14 里的实际输出。
//! 3.0 有意不同的只有一处：`over_base`（V4.5 保底用完后全站不空闲）整个机制已经删除，常量和短说明都不在了；
//! 历史记录里的 `over_base` 按没有码处理，显示 detail 的第一句。

use gate_core::autopilot::{KEY_GUARD_SKIP_REASONS, MEMBER_CAUSED};
use gate_core::reasons::{self, Reason};
use std::collections::HashSet;

/// 全部原因码：(常量名, 码, 给站长看的短说明)。`None` = 没有短说明，显示 detail 的第一句。
const ALL: &[(&str, &str, Option<&str>)] = &[
    ("KEY_PAUSED", reasons::KEY_PAUSED, Some("Key 被暂停")),
    ("QUEUE_FULL", reasons::QUEUE_FULL, Some("排队满")),
    ("KEY_BUSY", reasons::KEY_BUSY, Some("上一张没出完就重发")),
    (
        "MEMBER_QUEUE_FULL",
        reasons::MEMBER_QUEUE_FULL,
        Some("每人排队已满"),
    ),
    (
        "UPSTREAM_LOCKED",
        reasons::UPSTREAM_LOCKED,
        Some("上游并发锁（不算限流）"),
    ),
    ("HOURLY_CAP", reasons::HOURLY_CAP, Some("本小时全站上限")),
    ("QUIET_CAP", reasons::QUIET_CAP, Some("安静时段上限")),
    ("CAP_3H", reasons::CAP_3H, Some("3 小时全站上限")),
    ("BURST_CAP", reasons::BURST_CAP, Some("突发窗口限速")),
    ("DAILY_CAP", reasons::DAILY_CAP, Some("全站每日上限")),
    ("PACED", reasons::PACED, Some("全天平滑放行")),
    ("BREAKER", reasons::BREAKER, Some("全站熔断")),
    ("COOLDOWN", reasons::COOLDOWN, Some("上游限流暂停")),
    ("V5_EXHAUSTED", reasons::V5_EXHAUSTED, Some("全站 V5 用完")),
    ("QUEUE_TIMEOUT", reasons::QUEUE_TIMEOUT, Some("排队超时")),
    ("MEMBER_QUOTA", reasons::MEMBER_QUOTA, Some("个人额度用完")),
    ("SITE_QUOTA", reasons::SITE_QUOTA, Some("全站额度用完")),
    ("KEY_RATE", reasons::KEY_RATE, Some("Key 请求过快")),
    ("BODY_ABORT", reasons::BODY_ABORT, Some("请求体中断")),
    (
        "TEST_REQUEST",
        reasons::TEST_REQUEST,
        Some("连接测试（未出图）"),
    ),
    ("FEATURE_OFF", reasons::FEATURE_OFF, None),
    ("PARAM", reasons::PARAM, None),
    ("KEY_DISABLED", reasons::KEY_DISABLED, Some("Key 已停用")),
    ("KEY_INVALID", reasons::KEY_INVALID, Some("Key 无效")),
];

#[test]
fn codes_are_the_frozen_snake_case_names() {
    // 码是写进库的字符串，改一个字就和历史记录对不上
    for (name, code, _) in ALL {
        assert_eq!(*code, name.to_lowercase(), "{name}");
    }
    let mut codes: Vec<_> = ALL.iter().map(|(_, c, _)| *c).collect();
    codes.sort_unstable();
    codes.dedup();
    assert_eq!(codes.len(), ALL.len(), "原因码不能重复");
    assert_eq!(ALL.len(), 24);
}

#[test]
fn table_covers_every_code_declared_in_the_module() {
    // 扫描 reasons.rs 里 `pub const X: &str = "x"`，新增 / 删除原因码时这张表必须跟着改
    let mut declared: Vec<(String, String)> = include_str!("../src/reasons.rs")
        .lines()
        .filter_map(|line| {
            let rest = line.strip_prefix("pub const ")?;
            let (name, rest) = rest.split_once(": &str = \"")?;
            let (code, _) = rest.split_once('"')?;
            Some((name.to_string(), code.to_string()))
        })
        .collect();
    let mut table: Vec<(String, String)> = ALL
        .iter()
        .map(|(name, code, _)| (name.to_string(), code.to_string()))
        .collect();
    declared.sort();
    table.sort();
    assert_eq!(declared, table);
}

#[test]
fn every_code_has_a_short_label_or_falls_back_to_detail() {
    let labeled = ALL.iter().filter(|(_, _, l)| l.is_some()).count();
    assert_eq!(reasons::LABELS.len(), labeled);
    for (name, code, label) in ALL {
        match label {
            Some(label) => {
                // 有短说明：用码，detail 写什么都不看
                assert!(
                    reasons::LABELS.contains(&(*code, *label)),
                    "{name} 不在 LABELS 里"
                );
                assert_eq!(reasons::reason_label(code, ""), *label, "{name}");
                assert_eq!(
                    reasons::reason_label(code, "429 随便什么说明，别的"),
                    *label,
                    "{name}"
                );
            }
            None => {
                // 没有短说明：与没有码一样，显示 detail 的第一句
                assert!(reasons::LABELS.iter().all(|(c, _)| c != code), "{name}");
                assert_eq!(reasons::reason_label(code, ""), "其他", "{name}");
                assert_eq!(
                    reasons::reason_label(code, "403 暂无权使用，联系站长"),
                    "暂无权使用",
                    "{name}"
                );
            }
        }
    }
}

#[test]
fn label_falls_back_to_the_first_clause_of_detail() {
    // (码, detail, 短说明)。没有码（或码没有短说明）时：去掉开头的 "NNN "（三个十进制数字加空白），
    // 取第一个分句（（ ( ， , 。 ： : ； ; 之前），去首尾空白；超过 18 个字符截断加「…」；空串显示「其他」
    let table: &[(&str, &str, &str)] = &[
        ("", "429 无关的说明", "无关的说明"),
        ("", "429无关的说明", "无关的说明"),
        ("", "429", "其他"),
        ("", "503", "其他"),
        ("", "50", "50"),
        ("", "5", "5"),
        ("", " 429 ", "429"),
        ("", "４２９ 全角数字开头", "全角数字开头"),
        ("", "٤٢٩ 阿拉伯数字开头", "阿拉伯数字开头"),
        ("", "4290 多一位", "0 多一位"),
        ("", "429\u{3000}全角空格", "全角空格"),
        ("", "429\n换行", "换行"),
        ("", "429\u{1c}控制符", "控制符"),
        ("", "429 \t 多个空白", "多个空白"),
        ("", "42 9 不是连续三位", "42 9 不是连续三位"),
        ("", "５429）429xxxxxxxxxx（）", "9）429xxxxxxxxxx"),
        ("", "429 \u{3000} \n正文", "正文"),
        ("", "429 429 重复", "429 重复"),
        ("", "abc 429 在中间", "abc 429 在中间"),
        ("", "第一句，第二句", "第一句"),
        ("", "甲（乙", "甲"),
        ("", "甲(乙", "甲"),
        ("", "甲,乙", "甲"),
        ("", "甲。乙", "甲"),
        ("", "甲：乙", "甲"),
        ("", "甲:乙", "甲"),
        ("", "甲；乙", "甲"),
        ("", "甲;乙", "甲"),
        ("", "429 (括号里的)", "其他"),
        ("", "429 （全角括号）", "其他"),
        ("", "429，逗号", "其他"),
        ("", "429 。句号", "其他"),
        ("", "：开头冒号", "其他"),
        ("", "(开头括号)内容", "其他"),
        ("", "429 ，", "其他"),
        ("", "，", "其他"),
        ("", "甲、乙", "甲、乙"),
        ("", "甲 乙", "甲 乙"),
        ("", "甲.乙", "甲.乙"),
        ("", "甲）乙", "甲）乙"),
        ("", "甲!乙", "甲!乙"),
        ("", "甲/乙", "甲/乙"),
        ("", "  甲  ，乙", "甲"),
        ("", "429   甲 \u{3000} ，乙", "甲"),
        ("", "", "其他"),
        ("", "   ", "其他"),
        ("", "\t\n", "其他"),
        ("", "\u{3000}", "其他"),
        ("", "\u{200b}", "\u{200b}"),
        (
            "",
            "一二三四五六七八九十一二三四五六七八",
            "一二三四五六七八九十一二三四五六七八",
        ),
        (
            "",
            "一二三四五六七八九十一二三四五六七八九",
            "一二三四五六七八九十一二三四五六七八…",
        ),
        (
            "",
            "一二三四五六七八九十一二三四五六七八九十，后面",
            "一二三四五六七八九十一二三四五六七八…",
        ),
        ("", "abcdefghijklmnopqr", "abcdefghijklmnopqr"),
        ("", "abcdefghijklmnopqrs", "abcdefghijklmnopqr…"),
        ("", "abcdefghijklmnopqrstuvwxyz", "abcdefghijklmnopqr…"),
        ("", "abcdefghijklmnopq rst", "abcdefghijklmnopq …"),
        (
            "",
            "短句，长长长长长长长长长长长长长长长长长长长长长长长长长长长长长长",
            "短句",
        ),
        (
            "",
            "🦉🦉🦉🦉🦉🦉🦉🦉🦉🦉🦉🦉🦉🦉🦉🦉🦉🦉",
            "🦉🦉🦉🦉🦉🦉🦉🦉🦉🦉🦉🦉🦉🦉🦉🦉🦉🦉",
        ),
        (
            "",
            "🦉🦉🦉🦉🦉🦉🦉🦉🦉🦉🦉🦉🦉🦉🦉🦉🦉🦉🦉",
            "🦉🦉🦉🦉🦉🦉🦉🦉🦉🦉🦉🦉🦉🦉🦉🦉🦉🦉…",
        ),
        (
            "",
            "e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}",
            "e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}",
        ),
        (
            "",
            "e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}",
            "e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}…",
        ),
        (
            "",
            "429 字字字字字字字字字字字字字字字字字字",
            "字字字字字字字字字字字字字字字字字字",
        ),
        (
            "",
            "429 字字字字字字字字字字字字字字字字字字字",
            "字字字字字字字字字字字字字字字字字字…",
        ),
        ("queue_full", "429 whatever", "排队满"),
        ("queue_full", "", "排队满"),
        ("key_paused", "403 暂停了", "Key 被暂停"),
        ("cap_3h", "", "3 小时全站上限"),
        ("burst_cap", "429 x", "突发窗口限速"),
        ("paced", "429 全天平滑放行的说明", "全天平滑放行"),
        ("feature_off", "403 暂无权使用，联系站长", "暂无权使用"),
        ("feature_off", "", "其他"),
        (
            "param",
            "400 图片参数无效：width 必须是非负整数",
            "图片参数无效",
        ),
        ("param", "", "其他"),
        ("unknown", "403 暂无权使用，联系站长", "暂无权使用"),
        ("unknown", "", "其他"),
        ("CAP_3H", "429 大写不认", "大写不认"),
        ("key_paused ", "429 带空格不认", "带空格不认"),
        (" key_paused", "429 带空格不认", "带空格不认"),
        ("QUEUE_FULL", "", "其他"),
    ];
    for &(code, detail, want) in table {
        assert_eq!(
            reasons::reason_label(code, detail),
            want,
            "reason_label({code:?}, {detail:?})"
        );
    }
}

#[test]
fn removed_over_base_code_shows_the_first_clause() {
    // 历史记录里还有 over_base：没有短说明，和没有码时一样
    let table: &[(&str, &str, &str)] = &[
        (
            "over_base",
            "429 今天的保底 100 张已用完，请明天再试",
            "今天的保底 100 张已用完",
        ),
        (
            "over_base",
            "429 保底 50 张已用完（全站不空闲）",
            "保底 50 张已用完",
        ),
        ("over_base", "", "其他"),
    ];
    for &(code, detail, want) in table {
        assert_eq!(reasons::reason_label(code, detail), want, "{detail:?}");
        assert_eq!(reasons::reason_label("", detail), want, "{detail:?}");
    }
    assert!(reasons::LABELS.iter().all(|(c, _)| *c != "over_base"));
}

#[test]
fn reason_carries_code_like_a_str_subclass() {
    // 旧版 Reason 是 str 子类，带 .code；普通 str 没有 .code，code_of 给 ""
    let table: &[(&str, &str, &str)] = &[
        ("排队的人太多", reasons::QUEUE_FULL, "queue_full"),
        ("", reasons::PACED, "paced"),
        ("x", "", ""),
        ("说明 BODY_ABORT", reasons::BODY_ABORT, "body_abort"),
        ("说明 FEATURE_OFF", reasons::FEATURE_OFF, "feature_off"),
    ];
    for &(text, code, want) in table {
        let r = Reason::new(text, code);
        assert_eq!(r.code(), want);
        assert_eq!(r.text(), text);
        // 当字符串用：Deref、Display 都是说明文字
        assert_eq!(&*r, text);
        assert_eq!(r.to_string(), text);
        assert_eq!(r.len(), text.len());
        assert_eq!(r.clone(), r);
    }
    for text in ["普通字符串", ""] {
        let r = Reason::plain(text);
        assert_eq!(r.code(), "");
        assert_eq!(r, Reason::new(text, ""));
    }
    // 说明相同、码不同的两个 Reason 不相等
    assert_ne!(
        Reason::new("x", reasons::PACED),
        Reason::new("x", reasons::BREAKER)
    );
    assert_ne!(Reason::new("x", reasons::PACED), Reason::plain("x"));
}

#[test]
fn backfill_patterns_are_ordered_so_that_each_one_can_fire() {
    let known: Vec<_> = ALL.iter().map(|(_, c, _)| *c).collect();
    for (code, pattern) in reasons::BACKFILL {
        assert!(known.contains(code), "{pattern} 指向未知原因码 {code}");
        assert!(
            pattern.starts_with('%') && pattern.ends_with('%') && pattern.len() > 2,
            "{pattern}"
        );
        assert!(!pattern.trim_matches('%').contains(['%', '_']), "{pattern}");
        // 每条模式的样本文案都要落到它自己的码上：前面的模式不能把它吃掉（站点级额度在个人额度之前）
        let sample = pattern.trim_matches('%');
        assert_eq!(
            reasons::classify(sample),
            *code,
            "{pattern} 被前面的模式抢先匹配"
        );
    }
}

#[test]
fn backfill_classifies_historical_messages() {
    // (旧记录里的文案, 回填的原因码；"" = 不回填)
    let table: &[(&str, &str)] = &[
        ("429 排队的人太多，请稍后再试", "queue_full"),
        ("本小时出图量已达上限", "hourly_cap"),
        ("本时段出图已达上限", "burst_cap"),
        ("Key 已被暂停，请联系站长", "key_paused"),
        ("全站今日 V5 额度已用完", "site_quota"),
        ("全站今日 Vibe 额度已抢完", "site_quota"),
        ("全站今日 Anlas 额度已抢完", "site_quota"),
        ("全站本月 Anlas 预算已耗尽", "site_quota"),
        ("今日 V4.5 免费图额度已用完", "member_quota"),
        ("已达今日 V5 额度（每日 N 张）", "member_quota"),
        ("今日 Anlas 额度不足", "member_quota"),
        ("本月 Anlas 额度不足", "member_quota"),
        ("排队超时", "queue_timeout"),
        ("等待超时（预算核对）", "queue_timeout"),
        ("请求过于频繁", "key_rate"),
        ("同时进行中的请求过多", "key_rate"),
        ("请求体传输中断", "body_abort"),
        ("免费图额度已用完（V4.5 及以下）", "member_quota"),
        ("暂无权使用该功能", "feature_off"),
        ("站长暂未开放此功能", "feature_off"),
        ("本站只提供免费出图", "feature_off"),
        ("上一张图还没出完", "key_busy"),
        (
            "你已有 2 张在排队，等前面的出完再发（这张没有发出，没有扣费）",
            "member_queue_full",
        ),
        (
            "NovelAI 那边这个账号的上一张还没释放，重试几次仍没轮上",
            "upstream_locked",
        ),
        ("模型权限：仅允许 nai-diffusion-4-5-full", "feature_off"),
        ("img2img 未开放", "feature_off"),
        ("未列入本站图片模型白名单", "feature_off"),
        ("不支持 V4/V4.5/V5 模型的角色提示词", "param"),
        ("角色太多", "param"),
        ("异常权重", "param"),
        ("width 必须是非负整数", "param"),
        ("权重括号嵌套过深", "param"),
        (
            "现在是夜间安静时段（3:00–7:00），本站暂停出图，没有扣费",
            "quiet_cap",
        ),
        (
            "全站今天的 V5 额度被用完了（全站每天 300 张，先到先得）",
            "site_quota",
        ),
        ("你今天的 V5 额度用完了（每天 20 张）", "member_quota"),
        (
            "这把 Key 已过期，可以在 Discord 用 /register 重新领取",
            "key_disabled",
        ),
        ("今天的保底 100 张已用完", ""),
        ("没有任何已知片段的文案", ""),
        ("", ""),
    ];
    for &(text, want) in table {
        assert_eq!(reasons::classify(text), want, "{text:?}");
    }
}

#[test]
fn every_reason_code_is_classified_exactly_once() {
    let skip: HashSet<&str> = KEY_GUARD_SKIP_REASONS.into_iter().collect();
    let member: HashSet<&str> = MEMBER_CAUSED.into_iter().collect();
    assert!(skip.is_disjoint(&member), "同一个原因码不能两边都算");
    for (_, c, _) in ALL {
        assert!(
            skip.contains(c) || member.contains(c),
            "原因码 {c} 没有归类：加进 KEY_GUARD_SKIP_REASONS（站点层面）或 MEMBER_CAUSED（成员自己的问题）"
        );
    }
    for c in skip.iter().chain(member.iter()) {
        assert!(ALL.iter().any(|(_, x, _)| x == c), "{c} 不是现有的原因码");
    }
}
