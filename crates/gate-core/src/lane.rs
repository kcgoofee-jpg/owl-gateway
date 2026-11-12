//! 出图排队的两条车道。选号、换号、同一把 Key 里谁先发、首页「您排在」都按这个顺序。
//!
//! 数字越小越先轮到。V5 短作业车道（`Lane::Fast` / `guard_v5_priority`）已删除，归档在
//! GitHub tag `archive/v5-priority-lane`。V5 和 V4.5 一样走普通车道。

/// 一张等待中的图属于哪条车道。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
#[repr(u8)]
pub enum Lane {
    /// 站长号 / 赞助席插队：排在所有等待中的生图前面。不抢占已经发往上游的那张。
    Jump = 0,
    /// 普通（V4.5、V5，按先来后到）。
    #[default]
    Normal = 1,
}

impl Lane {
    /// `jump` 来自成员（站长号 / 赞助席）。判定全在服务端，客户端改不了。
    pub const fn of(jump: bool) -> Self {
        if jump { Self::Jump } else { Self::Normal }
    }

    pub const fn is_jump(self) -> bool {
        matches!(self, Self::Jump)
    }
}

#[cfg(test)]
mod tests {
    use super::Lane;

    #[test]
    fn jump_wins_over_plain() {
        assert_eq!(Lane::of(true), Lane::Jump);
        assert_eq!(Lane::of(false), Lane::Normal);
        assert!(Lane::Jump < Lane::Normal);
    }
}
