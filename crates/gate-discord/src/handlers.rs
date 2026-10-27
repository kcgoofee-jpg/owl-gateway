//! 命令回复文案：网关桥接响应 → 回复文本。全是纯函数，不碰 Discord / 网络，方便单测。
//!
//! 网关回的内容缺字段或形状不对时返回 [`HandlerError`]，由上层统一回成员「出了点问题……」并打 `[bug]` 日志。
//! 只在某个分支真的用到某个字段时才要求它存在（比如没有 V5 额度时不要求 `v5` 字段）。

use serde_json::{Map, Value};

use crate::events::Interaction;

// ---------- 文案常量 ----------

/// 命令出错时回给成员的话。
pub const GENERIC_ERROR: &str = "出了点问题，已自动记录，请稍后再试；一直不行请在频道里告诉站长。";
/// 连不上网关 / 返回体不是 JSON。
pub const BACKEND_UNAVAILABLE: &str = "服务暂时不可用，请稍后再试。";
/// 网关回了非 200，但响应里没有文字说明（`detail` 不是字符串，例如请求格式校验的错误列表）。
pub const BACKEND_FAILED: &str = "请求失败。";
pub const REGISTER_WRONG_GUILD: &str = "请在指定服务器使用 /register。";
pub const REGISTER_UNAVAILABLE: &str = "领 Key 暂不可用，请稍后再试。";
pub const REGISTER_DONE: &str =
    "领取成功。Key 已通过 Discord 私密回复发送；登录网站可查看额度，也能在右上角账户菜单复制 Key。";
pub const REGISTER_NOT_ELIGIBLE: &str = "暂时无法领取 Key。请查看这条私密回复中的具体原因；若提示先登录网站核验身份组，请完成后再使用 /register。";
/// 网关响应读不懂：缺字段，或字段的形状不对。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HandlerError {
    #[error("网关响应缺少字段 {0}")]
    Missing(String),
    #[error("网关响应格式不对：{0}")]
    Malformed(String),
}

/// 最多取前 `n` 个字符（按字符数，不会把汉字切半）。
pub fn clip(s: &str, n: usize) -> &str {
    s.char_indices().nth(n).map_or(s, |(i, _)| &s[..i])
}

fn obj(v: &Value) -> Result<&Map<String, Value>, HandlerError> {
    v.as_object()
        .ok_or_else(|| HandlerError::Malformed("不是 JSON 对象".into()))
}

/// 必填字段。
fn key<'a>(d: &'a Map<String, Value>, k: &str) -> Result<&'a Value, HandlerError> {
    d.get(k).ok_or_else(|| HandlerError::Missing(k.to_owned()))
}

/// 值算不算「有内容」：null / false / 0 / 空串 / 空列表 / 空对象都算没有。
fn enabled(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|x| x != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// 把网关给的值拼进文案：字符串原样，其余（数字等）用 JSON 写法。
fn show(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

// ---------- 网关响应 ----------

/// 一次网关调用的结果 `(status, data)`：200 时 `data` 是网关的 JSON；
/// 其他状态时 `data` 是 `detail`（取不到则「请求失败。」）；连不上 / 非 JSON 时是 `(0, 服务暂时不可用……)`。
#[derive(Debug, Clone, PartialEq)]
pub struct Backend {
    pub status: u16,
    pub data: Value,
}

impl Backend {
    pub fn unavailable() -> Backend {
        Backend {
            status: 0,
            data: Value::String(BACKEND_UNAVAILABLE.into()),
        }
    }

    pub fn is_ok(&self) -> bool {
        self.status == 200
    }

    /// 非 200 时发给成员的文字：网关的 `detail` 是字符串就原样给，否则给固定的「请求失败。」。
    pub fn text(&self) -> String {
        match &self.data {
            Value::String(s) => s.clone(),
            _ => BACKEND_FAILED.to_owned(),
        }
    }
}

/// 把网关的 HTTP 响应翻译成 [`Backend`]。
/// 先解析 JSON（失败 = 服务暂时不可用，哪怕状态码是 502），再看状态码；
/// 非 200 时取 `detail`，响应不是 JSON 对象则算读不懂。
pub fn interpret_backend(status: u16, body: &[u8]) -> Result<Backend, HandlerError> {
    let Ok(data) = serde_json::from_slice::<Value>(body) else {
        return Ok(Backend::unavailable());
    };
    if status == 200 {
        return Ok(Backend { status, data });
    }
    let detail = obj(&data)?
        .get("detail")
        .cloned()
        .unwrap_or_else(|| Value::String(BACKEND_FAILED.into()));
    Ok(Backend {
        status,
        data: detail,
    })
}

// ---------- /resetkey ----------

pub fn resetkey_text(data: &Value) -> Result<String, HandlerError> {
    let k = key(obj(data)?, "key")?;
    Ok(format!(
        "新的 Key（只有你能看到这条消息，请妥善保存）：\n`{}`\n旧 Key 已失效。",
        show(k)
    ))
}

// ---------- /register ----------

/// `/register` 发给 `/self-register/issue` 的 JSON。
/// `invite_key` 是已经去掉首尾空白的 /register 选项。
pub fn register_payload(it: &Interaction, invite_key: &str) -> Result<Value, HandlerError> {
    let user = it
        .actor()
        .ok_or_else(|| HandlerError::Malformed("交互里没有用户".into()))?;
    let name = clip(&user.username, 80);
    // 显示名：全局昵称，其次服务器昵称，最后用户名
    let global_name = user
        .global_name
        .as_deref()
        .filter(|s| !s.is_empty())
        .or_else(|| it.nick().filter(|s| !s.is_empty()))
        .or(Some(user.username.as_str()).filter(|s| !s.is_empty()))
        .unwrap_or("");
    let avatar = user.avatar.as_deref().unwrap_or("");
    Ok(serde_json::json!({
        "discord_id": user.id,
        "guild_id": it.guild_id_str(),
        "name": name,
        "username": name,
        "global_name": clip(global_name, 80),
        "avatar": clip(avatar, 120),
        "invite_key": clip(invite_key, 80),
    }))
}

/// `/register` 按网关 HTTP 响应决定回复。网络层失败（连不上 / 超时）由调用方直接用 [`REGISTER_UNAVAILABLE`]。
pub fn register_message(status: u16, body: &[u8]) -> Result<String, HandlerError> {
    match status {
        200 | 403 => {
            // 只有 200 / 403 才解析 JSON；解析失败按「暂不可用」回
            let Ok(data) = serde_json::from_slice::<Value>(body) else {
                return Ok(REGISTER_UNAVAILABLE.to_owned());
            };
            let d = obj(&data)?;
            if status == 200 {
                Ok(match d.get("message").filter(|v| enabled(v)) {
                    Some(m) => show(m),
                    None => REGISTER_DONE.to_owned(),
                })
            } else {
                Ok(d.get("detail")
                    .and_then(Value::as_str)
                    .map_or_else(|| REGISTER_NOT_ELIGIBLE.to_owned(), str::to_owned))
            }
        }
        _ => Ok(REGISTER_UNAVAILABLE.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn interaction(user: Value, member_extra: Value) -> Interaction {
        serde_json::from_value(json!({
            "id": "900000000000000010", "application_id": "900000000000000003", "type": 2, "token": "t",
            "guild_id": "900000000000000001",
            "member": {"user": user, "nick": member_extra},
            "data": {"name": "register"},
        }))
        .unwrap()
    }

    #[test]
    fn resetkey_text_shows_the_key_once() {
        assert_eq!(
            resetkey_text(&json!({"key": "nai-abc"})).unwrap(),
            "新的 Key（只有你能看到这条消息，请妥善保存）：\n`nai-abc`\n旧 Key 已失效。"
        );
        assert!(resetkey_text(&json!({})).is_err());
    }

    #[test]
    fn interpret_backend_branches() {
        let ok = interpret_backend(200, br#"{"a":1}"#).unwrap();
        assert_eq!(
            ok,
            Backend {
                status: 200,
                data: json!({"a": 1})
            }
        );
        let denied = interpret_backend(
            403,
            "{\"detail\":\"请在指定服务器使用该命令。\"}".as_bytes(),
        )
        .unwrap();
        assert_eq!(denied.text(), "请在指定服务器使用该命令。");
        // 非 200 又没有 detail
        assert_eq!(
            interpret_backend(500, b"{}").unwrap().text(),
            BACKEND_FAILED
        );
        // 不是 JSON：不管状态码，都是 (0, 服务暂时不可用)
        assert_eq!(
            interpret_backend(502, b"<html>").unwrap(),
            Backend::unavailable()
        );
        assert_eq!(interpret_backend(200, b"").unwrap(), Backend::unavailable());
        // 非 200 且 JSON 不是对象：读不懂
        assert!(interpret_backend(422, b"[1]").is_err());
        // 请求格式校验的错误：detail 是列表，不把内部结构给成员看，给固定文案
        let v = interpret_backend(422, br#"{"detail":[{"msg":"x"}]}"#).unwrap();
        assert_eq!(v.text(), BACKEND_FAILED);
    }

    #[test]
    fn register_messages() {
        assert_eq!(
            register_message(200, "{\"message\":\"Key: nai-x\"}".as_bytes()).unwrap(),
            "Key: nai-x"
        );
        assert_eq!(register_message(200, b"{}").unwrap(), REGISTER_DONE);
        assert_eq!(
            register_message(200, br#"{"message":""}"#).unwrap(),
            REGISTER_DONE
        );
        assert_eq!(
            register_message(403, "{\"detail\":\"账号太新\"}".as_bytes()).unwrap(),
            "账号太新"
        );
        assert_eq!(register_message(403, b"{}").unwrap(), REGISTER_NOT_ELIGIBLE);
        // detail 不是字符串：固定文案
        assert_eq!(
            register_message(403, br#"{"detail":[1]}"#).unwrap(),
            REGISTER_NOT_ELIGIBLE
        );
        assert_eq!(
            register_message(200, b"nope").unwrap(),
            REGISTER_UNAVAILABLE
        );
        assert_eq!(
            register_message(403, b"nope").unwrap(),
            REGISTER_UNAVAILABLE
        );
        assert_eq!(register_message(500, b"{}").unwrap(), REGISTER_UNAVAILABLE);
        assert_eq!(
            register_message(401, b"{\"detail\":\"x\"}").unwrap(),
            REGISTER_UNAVAILABLE
        );
        assert!(register_message(200, b"[1]").is_err());
    }

    #[test]
    fn register_payload_fields_and_order() {
        let it = interaction(
            json!({"id": "900000000000000005", "username": "alice", "global_name": "Alice", "avatar": "abc123"}),
            Value::Null,
        );
        let p = register_payload(&it, "邀请码").unwrap();
        assert_eq!(
            p.to_string(),
            r#"{"discord_id":"900000000000000005","guild_id":"900000000000000001","name":"alice","username":"alice","global_name":"Alice","avatar":"abc123","invite_key":"邀请码"}"#
        );
    }

    #[test]
    fn register_payload_global_name_fallbacks() {
        // 没有 global_name：用服务器昵称
        let it = interaction(
            json!({"id": "900000000000000005", "username": "alice", "global_name": null, "avatar": null}),
            json!("昵称"),
        );
        let p = register_payload(&it, "").unwrap();
        assert_eq!(p["global_name"], "昵称");
        assert_eq!(p["avatar"], "");
        let it = interaction(
            json!({"id": "900000000000000005", "username": "alice"}),
            Value::Null,
        );
        assert_eq!(register_payload(&it, "").unwrap()["global_name"], "alice");
    }

    #[test]
    fn register_payload_truncates_by_code_points() {
        let it = interaction(
            json!({"id": "900000000000000005", "username": "名".repeat(100), "avatar": "a".repeat(200)}),
            Value::Null,
        );
        let p = register_payload(&it, &"k".repeat(100)).unwrap();
        assert_eq!(p["name"].as_str().unwrap().chars().count(), 80);
        assert_eq!(p["avatar"].as_str().unwrap().len(), 120);
        assert_eq!(p["invite_key"].as_str().unwrap().len(), 80);
    }
}
