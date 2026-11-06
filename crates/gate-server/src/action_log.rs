//! 后台操作日志：谁在什么时候做了什么。写入失败只打印警告，绝不影响操作本身。
//! 参数摘要去掉 Token、密码和公告正文等敏感内容。

use gate_core::primitives::take_chars;
use gate_db::Db;
use serde_json::{Map, Value};

const SECRET_FIELDS: [&str; 9] = [
    "token",
    "password",
    "current",
    "new",
    "current_password",
    "new_password",
    "old_password",
    "html",
    "secret",
];

/// (方法, 路由模板) → 操作名；模板不含 /admin/api 前缀。
pub const ADMIN_ACTIONS: &[(&str, &str, &str)] = &[
    ("POST", "/login", "登录后台"),
    ("PUT", "/password", "修改后台密码"),
    ("POST", "/logout", "退出登录"),
    ("POST", "/keys", "新建 Key"),
    ("POST", "/keys/{key_id}/regenerate", "重新生成 Key"),
    ("PATCH", "/keys/{key_id}", "修改 Key"),
    (
        "POST",
        "/keys/{key_id}/reset-daily-image-quota",
        "重置今日额度",
    ),
    ("DELETE", "/keys/{key_id}", "删除 Key"),
    ("PUT", "/keys/{key_id}/tags", "成员标签"),
    ("POST", "/keys/{key_id}/coupons", "发重置券"),
    ("DELETE", "/keys/{key_id}/tags/{tag}", "移除成员标签"),
    ("PUT", "/runtime-limits", "修改排队与冷却"),
    ("POST", "/upstream-tokens", "添加上游 Token"),
    ("PUT", "/upstream-tokens/{token_id}", "替换上游 Token"),
    ("DELETE", "/upstream-tokens/{token_id}", "删除上游 Token"),
    (
        "PUT",
        "/upstream-tokens/{token_id}/v5-limit",
        "修改上游 Token V5 日限",
    ),
    (
        "PUT",
        "/upstream-tokens/{token_id}/enabled",
        "启用/停用上游 Token",
    ),
    (
        "PUT",
        "/members/{discord_id}/image-access",
        "切换站长生图权限",
    ),
    ("PUT", "/settings", "修改预算"),
    ("POST", "/ops/manual", "手动改设置"),
    ("PUT", "/ops/registration", "修改开放注册设置"),
    ("PUT", "/guard", "修改账号保护与排队设置"),
    ("POST", "/ops/reset-all-keys", "全站杀头重置"),
    ("POST", "/keys/{key_id}/share-clear", "解除限流暂停"),
];

/// 操作名；表里没有的写成「方法 路径」。
pub fn label(method: &str, path: &str) -> String {
    ADMIN_ACTIONS
        .iter()
        .find(|(m, p, _)| *m == method && *p == path)
        .map_or_else(|| format!("{method} {path}"), |(_, _, l)| (*l).to_string())
}

/// 把请求参数压成一行摘要，去掉敏感字段。
pub fn summarize(body: &Value, limit: usize) -> String {
    let Some(obj) = body.as_object().filter(|o| !o.is_empty()) else {
        return String::new();
    };
    let mut clean = Map::new();
    for (name, value) in obj {
        let v = if SECRET_FIELDS.contains(&name.to_lowercase().as_str()) {
            Value::String("（已隐去）".into())
        } else if let Value::String(s) = value
            && gate_core::primitives::len_chars(s) > 80
        {
            Value::String(format!("{}…", take_chars(s, 80)))
        } else {
            value.clone()
        };
        clean.insert(name.clone(), v);
    }
    let text = String::from_utf8(gate_core::json::wire(&Value::Object(clean))).unwrap_or_default();
    if gate_core::primitives::len_chars(&text) <= limit {
        text
    } else {
        format!("{}…", take_chars(&text, limit))
    }
}

/// 写一条操作日志；失败只打印警告。
pub async fn log_action(
    db: &Db,
    actor: &str,
    action: &str,
    target: &str,
    detail: &str,
    ok: bool,
    now: f64,
) {
    if let Err(e) = db
        .add_admin_action(
            take_chars(actor, 80),
            take_chars(action, 80),
            take_chars(target, 120),
            take_chars(detail, 400),
            ok,
            now,
        )
        .await
    {
        tracing::warn!("admin action log failed: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn summarize_hides_secrets() {
        let long = "长".repeat(90);
        let s = summarize(
            &json!({"name": "a", "Token": "nai-x", "html": "<p>", "note": long}),
            300,
        );
        assert_eq!(
            s,
            format!(
                "{{\"name\":\"a\",\"Token\":\"（已隐去）\",\"html\":\"（已隐去）\",\"note\":\"{}…\"}}",
                "长".repeat(80)
            )
        );
        assert_eq!(summarize(&json!({}), 300), "");
        assert_eq!(summarize(&json!([1]), 300), "");
    }
}
