//! V5 access policy. Identity is the stable Discord subject, never a Key id or display name.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Preserve the pre-policy behavior for existing deployments until explicitly migrated.
    Legacy,
    Closed,
    Restricted,
    Everyone,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Legacy => "legacy",
            Self::Closed => "closed",
            Self::Restricted => "restricted",
            Self::Everyone => "everyone",
        }
    }
}

pub fn mode_from_raw(raw: Option<&str>) -> Mode {
    match raw {
        None | Some("legacy") => Mode::Legacy,
        Some("closed") => Mode::Closed,
        Some("restricted") => Mode::Restricted,
        Some("everyone") => Mode::Everyone,
        Some(_) => Mode::Closed,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Decision {
    pub allowed: bool,
    pub reason: &'static str,
}

pub fn decide(mode: Mode, subject: Option<&str>, authorized: &[String]) -> Decision {
    let allowed = match mode {
        Mode::Legacy | Mode::Everyone => true,
        Mode::Closed => false,
        Mode::Restricted => subject.is_some_and(|id| authorized.iter().any(|v| v == id)),
    };
    Decision {
        allowed,
        reason: match (mode, allowed) {
            (Mode::Legacy, _) => "legacy policy",
            (_, true) => "authorized by V5 policy",
            (Mode::Closed, false) => "V5 access is closed",
            (Mode::Restricted, false) => "subject is not authorized for V5",
            (Mode::Everyone, false) => unreachable!(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn access_modes_and_stable_subject() {
        let ids = vec!["900000000000000001".to_string()];
        assert!(decide(Mode::Legacy, None, &ids).allowed);
        assert!(!decide(Mode::Closed, Some(&ids[0]), &ids).allowed);
        assert!(decide(Mode::Restricted, Some(&ids[0]), &ids).allowed);
        assert!(!decide(Mode::Restricted, Some("900000000000000002"), &ids).allowed);
        assert!(decide(Mode::Everyone, None, &[]).allowed);
        assert_eq!(mode_from_raw(None), Mode::Legacy);
        assert_eq!(mode_from_raw(Some("corrupt")), Mode::Closed);
    }
}
