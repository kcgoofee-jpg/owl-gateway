//! 首页额度圆环要显示的行。
//!
//! 个人剩余和全站剩余谁更小、算停用还是用完，只在这里算一次。`/public/me` 把结果放进
//! `today.rows`；页面只按 `reason` 填中文，不再自己取较小值。
//!
//! `reason` 为空表示还能用。非空时：
//! - `disabled`：这一档停用。个人上限是 0 时 `cap` 为 0；个人还有上限、但全站池是 0 时 `cap` 仍是个人上限。
//! - `member_off`：这名成员没开 V5。
//! - `site_paused`：站长暂停了 V5。
//! - `upstream_exhausted`：上游免费 V5 暂时用完。
//! - `site_empty`：全站池上限大于 0，但今天已经用完。

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Input {
    /// 今天的免费图张数。没有这份计数时用 `images - v5_used`。
    pub legacy: Option<i64>,
    pub images: i64,
    pub v5_used: i64,
    pub daily_images: i64,
    pub image_on: bool,
    pub daily_v5: i64,
    pub v5_on: bool,
    pub v5_paused: bool,
    /// 缺省按还有免费 V5（不误报用完）。
    pub upstream_v5_free: bool,
    pub site_v5_cap: i64,
    pub site_v5_used: i64,
    pub anlas_used: i64,
    pub daily_anlas: i64,
    pub site_anlas_cap: i64,
    pub site_anlas_left: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Row {
    pub id: &'static str,
    pub used: i64,
    pub cap: i64,
    pub left: i64,
    pub paused: bool,
    /// 空字符串表示没有暂停原因。
    pub reason: &'static str,
}

pub fn rows(input: &Input) -> Vec<Row> {
    let v5_used = input.v5_used.max(0);
    let legacy_used = input
        .legacy
        .unwrap_or_else(|| (input.images - v5_used).max(0))
        .max(0);
    let mut out = vec![legacy_row(legacy_used, input)];
    out.push(v5_row(v5_used, input));
    if input.daily_anlas > 0 {
        out.push(anlas_row(input));
    }
    out
}

fn open(id: &'static str, used: i64, cap: i64, left: i64) -> Row {
    Row {
        id,
        used,
        cap,
        left,
        paused: false,
        reason: "",
    }
}

fn paused(id: &'static str, used: i64, cap: i64, reason: &'static str) -> Row {
    Row {
        id,
        used,
        cap,
        left: 0,
        paused: true,
        reason,
    }
}

fn legacy_row(used: i64, input: &Input) -> Row {
    let cap = if input.image_on {
        input.daily_images.max(0)
    } else {
        0
    };
    if cap == 0 {
        paused("legacy", used, 0, "disabled")
    } else {
        open("legacy", used, cap, (cap - used).max(0))
    }
}

fn v5_row(used: i64, input: &Input) -> Row {
    let cap = input.daily_v5.max(0);
    if cap == 0 {
        return paused("v5", used, 0, "disabled");
    }
    if !input.v5_on {
        return paused("v5", used, cap, "member_off");
    }
    if input.v5_paused {
        return paused("v5", used, cap, "site_paused");
    }
    if !input.upstream_v5_free {
        return paused("v5", used, cap, "upstream_exhausted");
    }
    // 全站池 0 是停用，不是「今天用完了」。用完只在上限大于 0 且剩余为 0 时。
    if input.site_v5_cap <= 0 {
        return paused("v5", used, cap, "disabled");
    }
    let site_left = (input.site_v5_cap - input.site_v5_used).max(0);
    if site_left <= 0 {
        return paused("v5", used, cap, "site_empty");
    }
    let personal = (cap - used).max(0);
    open("v5", used, cap, personal.min(site_left))
}

fn anlas_row(input: &Input) -> Row {
    let used = input.anlas_used.max(0);
    let cap = input.daily_anlas.max(0);
    if input.site_anlas_cap <= 0 {
        return paused("anlas", used, cap, "disabled");
    }
    if input.site_anlas_left <= 0 {
        return paused("anlas", used, cap, "site_empty");
    }
    let personal = (cap - used).max(0);
    open(
        "anlas",
        used,
        cap,
        personal.min(input.site_anlas_left.max(0)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open_input() -> Input {
        Input {
            legacy: Some(20),
            images: 30,
            v5_used: 4,
            daily_images: 150,
            image_on: true,
            daily_v5: 20,
            v5_on: true,
            v5_paused: false,
            upstream_v5_free: true,
            site_v5_cap: 300,
            site_v5_used: 100,
            anlas_used: 12,
            daily_anlas: 50,
            site_anlas_cap: 200,
            site_anlas_left: 200,
        }
    }

    fn row<'a>(rows: &'a [Row], id: &str) -> &'a Row {
        rows.iter().find(|r| r.id == id).unwrap()
    }

    #[test]
    fn personal_and_site_remaining_take_the_smaller() {
        let got = rows(&open_input());
        assert_eq!(row(&got, "legacy").left, 130);
        let v5 = row(&got, "v5");
        assert_eq!((v5.used, v5.cap, v5.left, v5.paused), (4, 20, 16, false));
        let tight = Input {
            site_v5_used: 297,
            ..open_input()
        };
        assert_eq!(row(&rows(&tight), "v5").left, 3);
        let anlas = row(&got, "anlas");
        assert_eq!((anlas.left, anlas.paused), (38, false));
    }

    #[test]
    fn missing_legacy_falls_back_to_images_minus_v5() {
        let input = Input {
            legacy: None,
            ..open_input()
        };
        assert_eq!(row(&rows(&input), "legacy").used, 26);
    }

    #[test]
    fn zero_personal_caps_are_disabled() {
        let input = Input {
            daily_images: 0,
            daily_v5: 0,
            daily_anlas: 0,
            image_on: true,
            ..open_input()
        };
        let rows = rows(&input);
        assert_eq!(
            rows.iter().map(|r| r.id).collect::<Vec<_>>(),
            ["legacy", "v5"]
        );
        assert_eq!(row(&rows, "legacy").reason, "disabled");
        assert_eq!(row(&rows, "v5").reason, "disabled");
        assert_eq!(row(&rows, "v5").cap, 0);
    }

    #[test]
    fn image_off_disables_the_legacy_row() {
        let input = Input {
            image_on: false,
            daily_images: 150,
            ..open_input()
        };
        let got = rows(&input);
        let legacy = row(&got, "legacy");
        assert_eq!((legacy.cap, legacy.reason), (0, "disabled"));
    }

    #[test]
    fn v5_pause_reasons_keep_priority() {
        let member_off = Input {
            v5_on: false,
            v5_paused: true,
            upstream_v5_free: false,
            site_v5_cap: 0,
            ..open_input()
        };
        assert_eq!(row(&rows(&member_off), "v5").reason, "member_off");

        let paused = Input {
            v5_paused: true,
            upstream_v5_free: false,
            ..open_input()
        };
        assert_eq!(row(&rows(&paused), "v5").reason, "site_paused");

        let upstream = Input {
            upstream_v5_free: false,
            site_v5_used: 300,
            ..open_input()
        };
        assert_eq!(row(&rows(&upstream), "v5").reason, "upstream_exhausted");
    }

    #[test]
    fn site_pool_off_is_not_the_same_as_used_up() {
        let off = Input {
            site_v5_cap: 0,
            site_v5_used: 0,
            ..open_input()
        };
        let off_rows = rows(&off);
        let v5 = row(&off_rows, "v5");
        assert_eq!(
            (v5.paused, v5.left, v5.cap, v5.reason),
            (true, 0, 20, "disabled")
        );

        let empty = Input {
            site_v5_cap: 300,
            site_v5_used: 300,
            ..open_input()
        };
        assert_eq!(row(&rows(&empty), "v5").reason, "site_empty");

        let anlas_off = Input {
            site_anlas_cap: 0,
            site_anlas_left: 0,
            ..open_input()
        };
        assert_eq!(row(&rows(&anlas_off), "anlas").reason, "disabled");
        let anlas_empty = Input {
            site_anlas_left: 0,
            ..open_input()
        };
        assert_eq!(row(&rows(&anlas_empty), "anlas").reason, "site_empty");
    }
}
