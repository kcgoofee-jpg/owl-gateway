//! 出图排队的三条车道。选号、换号、同一把 Key 里谁先发、首页「您排在」都按这个顺序。
//!
//! 数字越小越先轮到。硬老化（等满整个排队超时）只把普通升到 V5，**不能**升到插队。

/// 一张等待中的图属于哪条车道。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
#[repr(u8)]
pub enum Lane {
    /// 站长号 / 赞助席插队：排在所有等待中的生图前面。不抢占已经发往上游的那张。
    Jump = 0,
    /// V5 短作业（`guard_v5_priority`）。等满整个排队超时的普通请求会升到这条。
    Fast = 1,
    /// 普通（V4.5 等）。
    #[default]
    Normal = 2,
}

impl Lane {
    /// `jump` 来自成员（站长号 / 赞助席），`fast` 来自模型（V5）。插队优先。
    pub const fn of(jump: bool, fast: bool) -> Self {
        if jump {
            Self::Jump
        } else if fast {
            Self::Fast
        } else {
            Self::Normal
        }
    }

    /// 调度此刻真正用的车道：硬老化把 V5 / 普通升到 V5，插队不动。
    pub const fn effective(self, hard: bool) -> Self {
        match self {
            Self::Jump => Self::Jump,
            Self::Fast => Self::Fast,
            Self::Normal if hard => Self::Fast,
            Self::Normal => Self::Normal,
        }
    }

    pub const fn is_jump(self) -> bool {
        matches!(self, Self::Jump)
    }
}

#[cfg(test)]
mod tests {
    use super::Lane;

    #[test]
    fn jump_wins_over_v5() {
        assert_eq!(Lane::of(true, true), Lane::Jump);
        assert_eq!(Lane::of(true, false), Lane::Jump);
        assert_eq!(Lane::of(false, true), Lane::Fast);
        assert_eq!(Lane::of(false, false), Lane::Normal);
    }

    #[test]
    fn hard_aging_cannot_enter_jump() {
        assert_eq!(Lane::Jump.effective(true), Lane::Jump);
        assert_eq!(Lane::Fast.effective(true), Lane::Fast);
        assert_eq!(Lane::Normal.effective(true), Lane::Fast);
        assert_eq!(Lane::Normal.effective(false), Lane::Normal);
    }

    #[test]
    fn order_is_jump_then_fast_then_normal() {
        assert!(Lane::Jump < Lane::Fast);
        assert!(Lane::Fast < Lane::Normal);
    }
}
