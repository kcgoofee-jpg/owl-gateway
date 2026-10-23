//! Key 的停用原因（3.0 新增）：`api_keys.disabled_reason` 的取值，
//! 以及成员拿停用的 Key 请求时看到的说明。以前一律是「该 Key 已被禁用」，被全站重置停掉的成员
//! 不知道该去重新领，客户端里一直留着旧 Key。

/// 全站重置（后台「全站重置 Key」）停掉的。
pub const SITE_RESET: &str = "site_reset";
/// 站长在后台手动停用的（没写原因时也按这个处理）。
pub const ADMIN: &str = "admin";

/// 停用的 Key 被拿来请求时的 403 说明。
pub fn disabled_message(reason: Option<&str>) -> &'static str {
    match reason {
        Some(SITE_RESET) => {
            "这把 Key 已在全站重置时失效。请在 Discord 服务器用 /register 重新领取，并替换客户端中的 Key。"
        }
        _ => "这把 Key 已被停用。如有疑问，请在 Discord 服务器联系站长。",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn site_reset_tells_member_to_register_again() {
        assert!(disabled_message(Some(SITE_RESET)).contains("/register"));
        assert_eq!(
            disabled_message(Some(ADMIN)),
            "这把 Key 已被停用。如有疑问，请在 Discord 服务器联系站长。"
        );
        assert_eq!(disabled_message(None), disabled_message(Some(ADMIN)));
        assert_eq!(
            disabled_message(Some("别的")),
            disabled_message(Some(ADMIN))
        );
    }
}
