//! V5 Medium 降档开关（手动）。
//!
//! 设置 `v5_medium_mode`（登记在 `site_flags::V5_MEDIUM_MODE`）：
//! - `off`：不动（默认）；
//! - `on`：V5 Full（含局部重绘）一律改走 Medium，在费用估算和额度准入之前改。
//!
//! 按账号余量自动切换的 `auto` 档站长 10-23 删除：库里存着 `auto` 的按 `off`（fail-closed）。

use crate::site_flags;
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Off,
    On,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Off => "off",
            Mode::On => "on",
        }
    }

    /// `site_flags` 解析出来的值 → 模式；认不出的（含已删除的 `auto`）一律 `Off`。
    pub fn from_flag(v: &Value) -> Self {
        match v.as_str() {
            Some("on") => Mode::On,
            _ => Mode::Off,
        }
    }

    /// 库里的原始文本 → 模式。
    pub fn from_raw(raw: Option<&str>) -> Self {
        Self::from_flag(&site_flags::get_from_raw(&site_flags::V5_MEDIUM_MODE, raw))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn mode_parsing_fails_closed() {
        assert_eq!(Mode::from_raw(None), Mode::Off);
        assert_eq!(Mode::from_raw(Some("on")), Mode::On);
        for bad in ["", "ON", "yes", "1", "medium", "auto", "auto2"] {
            assert_eq!(Mode::from_raw(Some(bad)), Mode::Off, "{bad}");
        }
        assert_eq!(Mode::from_flag(&json!(true)), Mode::Off);
        assert_eq!(Mode::from_flag(&Value::Null), Mode::Off);
    }

    #[test]
    fn only_full_and_its_inpainting_map_to_medium() {
        use crate::policy::v5_medium_of;
        assert_eq!(
            v5_medium_of("nai-diffusion-5-full"),
            Some("nai-diffusion-5-full-medium")
        );
        assert_eq!(
            v5_medium_of(" NAI-Diffusion-5-Full-Inpainting "),
            Some("nai-diffusion-5-full-medium-inpainting")
        );
        for other in [
            "nai-diffusion-5-full-medium",
            "nai-diffusion-5-full-medium-inpainting",
            "nai-diffusion-5-curated",
            "nai-diffusion-5",
            "nai-v5",
            "nai-diffusion-4-5-full",
            "",
        ] {
            assert_eq!(v5_medium_of(other), None, "{other}");
        }
    }
}
