//! `POST /admin/api/ops/manual`：运维（Claude）手动改线上设置的唯一入口（站长 10-20：站长和 Claude 的改动都算「手动」）。
//!
//! 后台页面上没有的临时设置（账号日上限临时调整、站付 V5）以前是直接改库，没有记录。
//! 这里校验 → 改设置 → 写操作日志放在同一个事务里（[`gate_db::Db::set_setting_logged`]），并让网关内存即时生效。
//! 用 `nai-ops manual` 调用：请求带 `x-owl-actor: claude`，操作者记「手动（Claude）」。
//!
//! 请求体：`{"key": <见 KEYS>, "value": <新值；null = 删除>, "why": "原因"}`；
//! （V5 临时加量 `quota_v5_bonus` 和改当天 V5 方案 `v5_plan` 随 V5 自动方案在 10-23 删除。）
//! 不带 `key` 时只记一笔（`action` 必填），给在别处做完、需要补记的改动用。

use super::super::{ActionLogged, actor};
use crate::body::read_json_body;
use crate::error::{ApiError, ApiResult, HttpError, ok_json};
use crate::session::AdminSession;
use crate::state::AppState;
use axum::Router;
use axum::extract::{Request, State};
use axum::routing::post;
use gate_core::{guard, paid_v5, pyc, pyjson, pyval};
use serde_json::{Map, Value, json};

const BODY_LIMIT: usize = 64 * 1024;
/// (名字, 操作日志里的操作名, 对象)。
pub const KEYS: [(&str, &str, &str); 2] = [
    (
        guard::TOKEN_DAILY_CAPS_KEY,
        "手动：账号日上限临时调整",
        "上游账号日上限",
    ),
    (paid_v5::KEY, "手动：站付 V5", "站付 V5"),
];

pub(super) fn router() -> Router<AppState> {
    Router::new().route("/admin/api/ops/manual", post(manual))
}

fn bad(msg: impl Into<String>) -> ApiError {
    HttpError::new(422, msg.into()).into()
}

fn non_neg_int(v: &Value, name: &str) -> Result<i64, ApiError> {
    pyval::strict_int(v)
        .filter(|n| *n >= 0)
        .ok_or_else(|| bad(format!("{name} 必须是非负整数")))
}

/// 每项都要有截止时间而且还没过（临时调整忘了收回，就会一直生效）。
fn until_ok(v: &Value, now: f64, name: &str) -> Result<(), ApiError> {
    match v.get("until").and_then(Value::as_f64) {
        Some(u) if u > now => Ok(()),
        _ => Err(bad(format!("{name} 需要 until（还没到的 Unix 秒）"))),
    }
}

/// 校验新值，返回要写进库的文本。账号必须是令牌池里有的。
fn validate(key: &str, v: &Value, now: f64, tokens: &[String]) -> Result<String, ApiError> {
    let obj = v
        .as_object()
        .ok_or_else(|| bad(format!("{key} 的值必须是 JSON 对象")))?;
    let known = |id: &str| -> Result<(), ApiError> {
        if tokens.iter().any(|t| t == id) {
            Ok(())
        } else {
            Err(bad(format!("令牌池里没有账号 {}", pyc::take_chars(id, 24))))
        }
    };
    match key {
        guard::TOKEN_DAILY_CAPS_KEY => {
            for (id, e) in obj {
                known(id)?;
                non_neg_int(e.get("cap").unwrap_or(&Value::Null), "cap")?;
                until_ok(e, now, id)?;
            }
        }
        paid_v5::KEY => {
            for (id, e) in obj {
                known(id)?;
                match e.get("max_anlas").and_then(Value::as_f64) {
                    Some(m) if m > 0.0 && m.is_finite() => {}
                    _ => return Err(bad("max_anlas 必须是正数")),
                }
                until_ok(e, now, id)?;
            }
        }
        _ => return Err(bad(format!("不支持的设置 {key}"))),
    }
    Ok(pyjson::dumps(v))
}

fn shown(v: Option<&str>) -> String {
    v.map_or_else(|| "(无)".into(), str::to_string)
}

async fn manual(State(state): State<AppState>, _s: AdminSession, req: Request) -> ApiResult {
    let (parts, body) = req.into_parts();
    let body = read_json_body(&parts.headers, body, BODY_LIMIT).await?;
    let who = actor(&state, &parts);
    let now = state.now();
    let text = |k: &str| {
        body.get(k)
            .and_then(Value::as_str)
            .map(|s| pyc::strip(s).to_string())
            .unwrap_or_default()
    };
    let why = text("why");
    if why.is_empty() {
        return Err(bad("why 不能为空：写清楚为什么改、谁拍板、什么时候收回"));
    }
    let key = text("key");
    if key.is_empty() {
        let action = text("action");
        if action.is_empty() {
            return Err(bad("只记一笔时 action 不能为空"));
        }
        let target = text("target");
        state
            .db
            .add_admin_action(
                &who,
                pyc::take_chars(&action, 60),
                pyc::take_chars(&target, 60),
                pyc::take_chars(&why, 600),
                true,
                now,
            )
            .await?;
        let mut resp = ok_json(&json!({"ok": true, "recorded": true}));
        resp.extensions_mut().insert(ActionLogged);
        return Ok(resp);
    }
    let Some((_, action, target)) = KEYS.iter().find(|(k, _, _)| *k == key) else {
        let names: Vec<&str> = KEYS.iter().map(|k| k.0).collect();
        return Err(bad(format!(
            "不支持的设置 {key}；可用：{}",
            names.join(" / ")
        )));
    };
    let value = body.get("value").cloned().unwrap_or(Value::Null);
    let (setting_key, new_text) = if value.is_null() {
        (key.as_str(), None)
    } else {
        let tokens: Vec<String> = state
            .nai
            .status_rows()
            .await
            .into_iter()
            .map(|(t, _)| t.token_id.clone())
            .collect();
        (key.as_str(), Some(validate(&key, &value, now, &tokens)?))
    };
    let shown_new = shown(new_text.as_deref());
    // 先读一次旧值只为写进记录；真正的读改写在事务里
    let before = state.db.get_setting(setting_key).await?;
    let detail = format!("{} → {shown_new}；{why}", shown(before.as_deref()));
    let old = state
        .db
        .set_setting_logged(
            setting_key,
            new_text.as_deref(),
            &who,
            action,
            target,
            pyc::take_chars(&detail, 600),
            now,
        )
        .await?;

    // 网关内存即时生效（以前改账号日上限要重启网关）
    if setting_key == guard::TOKEN_DAILY_CAPS_KEY {
        state.guard.with(|g| {
            g.token_daily_caps = guard::parse_token_daily_caps(new_text.as_deref());
        });
    }
    let mut out = Map::new();
    out.insert("ok".into(), json!(true));
    out.insert("key".into(), json!(setting_key));
    out.insert("before".into(), json!(old));
    out.insert("after".into(), json!(new_text));
    out.insert("note".into(), json!(""));
    let mut resp = ok_json(&Value::Object(out));
    resp.extensions_mut().insert(ActionLogged);
    Ok(resp)
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: &str = "token-aaaa";

    #[test]
    fn validates_values() {
        let toks = vec![T.to_string()];
        let ok = |k: &str, v: Value| validate(k, &v, 100.0, &toks).is_ok();
        assert!(ok(
            guard::TOKEN_DAILY_CAPS_KEY,
            json!({T: {"cap": 2200, "until": 200}})
        ));
        assert!(!ok(
            guard::TOKEN_DAILY_CAPS_KEY,
            json!({"token-zzzz": {"cap": 2200, "until": 200}})
        ));
        assert!(!ok(
            guard::TOKEN_DAILY_CAPS_KEY,
            json!({T: {"cap": -1, "until": 200}})
        ));
        assert!(ok(
            paid_v5::KEY,
            json!({T: {"max_anlas": 9700, "until": 200}})
        ));
        assert!(!ok(
            paid_v5::KEY,
            json!({T: {"max_anlas": 0, "until": 200}})
        ));
        assert!(!ok("guard_account_daily_cap", json!({})));
        assert!(!ok("quota_v5_bonus", json!({"daily": 400, "until": 200})));
        assert!(!ok(guard::TOKEN_DAILY_CAPS_KEY, json!([1])));
    }
}
