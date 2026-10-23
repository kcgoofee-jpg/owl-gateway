//! 操作日志 `admin_actions.actor` 的分类（站长 10-20）：站长和 Claude 做的改动都算「手动」，
//! 算法和定时任务做的才算「自动」；成员自己的操作不打标签。
//!
//! 只按前缀判断，历史上出现过的写法都列在下面（「后台 *.*.x.y」「站长指示」「Claude（运维）」……）。
//! 后台列表的筛选用 [`sql_where`] 生成同一套规则的 SQL 条件，两边不会对不上。

/// Claude 改线上设置时写进操作日志的操作者。
pub const CLAUDE: &str = "手动（Claude）";

/// 站长在后台、站长口头指示、Claude 运维：都是人拍板的改动。
pub const MANUAL_PREFIXES: [&str; 5] = ["站长", "后台", "手动", "Claude", "运维"];
/// 算法、定时任务、自动驾驶。
pub const AUTO_PREFIXES: [&str; 2] = ["系统", "自动驾驶"];
/// 成员自己（Discord 指令 / 网页）。
pub const MEMBER_PREFIXES: [&str; 2] = ["Discord:", "网页:"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Manual,
    Auto,
    Member,
    /// 认不出的写法：不打标签。
    Other,
}

impl Kind {
    /// 接口里的取值：`manual` / `auto` / `member` / `""`。
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Manual => "manual",
            Kind::Auto => "auto",
            Kind::Member => "member",
            Kind::Other => "",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "manual" => Some(Kind::Manual),
            "auto" => Some(Kind::Auto),
            "member" => Some(Kind::Member),
            _ => None,
        }
    }

    fn prefixes(self) -> &'static [&'static str] {
        match self {
            Kind::Manual => &MANUAL_PREFIXES,
            Kind::Auto => &AUTO_PREFIXES,
            Kind::Member => &MEMBER_PREFIXES,
            Kind::Other => &[],
        }
    }
}

pub fn kind(actor: &str) -> Kind {
    [Kind::Member, Kind::Auto, Kind::Manual]
        .into_iter()
        .find(|k| k.prefixes().iter().any(|p| actor.starts_with(p)))
        .unwrap_or(Kind::Other)
}

/// `actor` 列属于 `k` 的 SQL 条件（前缀里没有 `%` / `_` / `'`，可以直接拼）。
/// 前缀互不重叠，所以和 [`kind`] 的判断顺序无关。
pub fn sql_where(k: Kind) -> String {
    let parts: Vec<String> = k
        .prefixes()
        .iter()
        .map(|p| format!("actor LIKE '{p}%'"))
        .collect();
    if parts.is_empty() {
        "0".into()
    } else {
        format!("({})", parts.join(" OR "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_actors() {
        for a in [
            "站长",
            "站长指示",
            "后台",
            "后台 *.*.1.2",
            CLAUDE,
            "Claude（运维）",
            "Claude（运维，站长指示）",
            "运维",
        ] {
            assert_eq!(kind(a), Kind::Manual, "{a}");
        }
        for a in ["系统", "自动驾驶"] {
            assert_eq!(kind(a), Kind::Auto, "{a}");
        }
        for a in ["Discord:123", "网页:*.*.0.1"] {
            assert_eq!(kind(a), Kind::Member, "{a}");
        }
        assert_eq!(kind("别的"), Kind::Other);
        assert_eq!(kind(""), Kind::Other);
    }

    #[test]
    fn prefixes_do_not_overlap_and_are_sql_safe() {
        let all: Vec<&str> = MANUAL_PREFIXES
            .iter()
            .chain(&AUTO_PREFIXES)
            .chain(&MEMBER_PREFIXES)
            .copied()
            .collect();
        for a in &all {
            assert!(!a.contains(['%', '_', '\'']), "{a}");
            for b in &all {
                assert!(a == b || !a.starts_with(b), "{a} / {b}");
            }
        }
        assert_eq!(
            sql_where(Kind::Auto),
            "(actor LIKE '系统%' OR actor LIKE '自动驾驶%')"
        );
        assert_eq!(sql_where(Kind::Other), "0");
    }

    #[test]
    fn kind_round_trips() {
        for k in [Kind::Manual, Kind::Auto, Kind::Member] {
            assert_eq!(Kind::parse(k.as_str()), Some(k));
        }
        assert_eq!(Kind::parse(""), None);
    }
}
