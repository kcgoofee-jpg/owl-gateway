//! Pure banned-account association evidence. Only browser evidence can block claims.
use serde_json::{Value, json};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct BanInfo {
    pub name: String,
    pub reason: String,
}
pub const KIND: &str = "banned_link";
/// /register 拒绝时给成员看的话
pub const BLOCK_NOTICE: &str =
    "这个浏览器曾登录已封禁账号，暂不能领取 Key；如有误会请联系站长复核。";
/// 同浏览器：拦领 Key
pub const SCORE: f64 = 95.0;
/// 仅匹配设备指纹：只提醒
pub const DEVICE_SCORE: f64 = 60.0;

/// X 和某个封禁号是怎么关联上的。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Ways {
    pub browser: bool,
    /// 同一个设备指纹，且不是苹果设备
    pub device: bool,
    /// 同一个设备指纹，苹果设备（只提醒）
    pub apple_device: bool,
}

impl Ways {
    pub fn add_device(&mut self, apple: bool) {
        if apple {
            self.apple_device = true;
        } else {
            self.device = true;
        }
    }
    /// 拦不拦领 Key
    pub fn blocks(&self) -> bool {
        self.browser
    }
    fn label(&self) -> String {
        let mut v = Vec::new();
        if self.browser {
            v.push("同浏览器");
        }
        if self.device {
            v.push("同设备");
        }
        if self.apple_device && !self.device {
            v.push("同设备（苹果）");
        }
        v.join("、")
    }
}

/// 信号的 subject：两个 Discord ID 排序后用逗号连起来（和其他信号一样）。
pub fn subject(x: &str, banned: &str) -> String {
    let mut v = [x, banned];
    v.sort_unstable();
    v.join(",")
}

fn banned_name<'a>(banned: &'a str, info: &'a BanInfo) -> &'a str {
    if info.name.is_empty() {
        banned
    } else {
        &info.name
    }
}

/// 信号的分数和 detail。
pub fn signal(banned: &str, info: &BanInfo, ways: &Ways) -> (f64, Value) {
    let name = banned_name(banned, info);
    let (score, summary) = if ways.blocks() {
        (
            SCORE,
            format!(
                "和已封禁的「{name}」{}登录过，领 Key 已拦下（忽略 = 放行）",
                ways.label()
            ),
        )
    } else {
        (
            DEVICE_SCORE,
            format!(
                "和已封禁的「{name}」{}登录过（设备指纹可能相同，只提醒、不拦）",
                ways.label()
            ),
        )
    };
    let mut evidence = vec![
        json!({"k": "关联的封禁号", "v": name}),
        json!({"k": "方式", "v": ways.label()}),
    ];
    if !info.reason.is_empty() {
        evidence.push(json!({"k": "封禁理由", "v": info.reason}));
    }
    (score, json!({"summary": summary, "evidence": evidence}))
}

/// 新线索告警说明（不含 IP / Key）。
pub fn alert_text(x: &str, banned: &str, info: &BanInfo, ways: &Ways) -> String {
    let name = banned_name(banned, info);
    let tail = if ways.blocks() {
        "已拦下领 Key；到后台「待审」确认或忽略（忽略 = 放行）。"
    } else {
        "设备指纹可能相同，只提醒、不拦。"
    };
    format!(
        "Discord {x} 和已封禁的「{name}」（{banned}）{}网页登录过。{tail}",
        ways.label()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::abuse::{device_parts, is_apple};
    use serde_json::json;

    #[test]
    fn ways_label_and_blocking() {
        let mut w = Ways::default();
        w.add_device(true);
        assert!(!w.blocks());
        assert_eq!(w.label(), "同设备（苹果）");
        w.browser = true;
        assert!(w.blocks());
        assert_eq!(w.label(), "同浏览器、同设备（苹果）");
        w.add_device(false);
        assert_eq!(w.label(), "同浏览器、同设备");
        assert_eq!(subject("9", "10"), "10,9");
        let (score, d) = signal(
            "1",
            &BanInfo::default(),
            &Ways {
                device: true,
                ..Default::default()
            },
        );
        assert_eq!(score, DEVICE_SCORE);
        assert!(
            !Ways {
                device: true,
                ..Default::default()
            }
            .blocks()
        );
        assert_eq!(d["evidence"][0]["v"], "1");
    }

    #[test]
    fn apple_detection() {
        assert!(!is_apple(
            &json!({"gpuRenderer": "ANGLE (NVIDIA)", "platform": "Win32"})
        ));
        assert!(is_apple(
            &json!({"gpuRenderer": "Apple GPU", "platform": "iPhone"})
        ));
        assert!(is_apple(&device_parts(
            r#"{"id":"x","data":{"platform":"MacIntel"}}"#
        )));
    }
}
