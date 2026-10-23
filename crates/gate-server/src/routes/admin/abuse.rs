//! 小号 / 共享 Key 信号（`abuse_signals`）的后台接口：列出、确认或忽略（后台页面已删除，由 Claude 按站长指示调用）。
//! 确认 = 状态改成 confirmed，并给涉及的每把 Key 打手动标签「疑似小号」（note = 信号类型）。不自动封号。

use super::req::{AdminPath, AdminQuery, R, http, json_of};
use super::{actor, mark_logged};
use crate::action_log::log_action;
use crate::error::ApiError;
use crate::error::ok_json;
use crate::session::AdminSession;
use crate::state::AppState;
use axum::Router;
use axum::extract::{Request, State};
use axum::routing::{get, post};
use gate_core::pyval;
use gate_db::repo::abuse::{ABUSE_STATUSES, LoginScope};
use serde_json::{Value, json};

/// 确认后打在 Key 上的手动标签。
pub const SUSPECT_TAG: &str = "疑似小号";
const LIST_LIMIT: i64 = 200;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/admin/api/abuse", get(list))
        .route("/admin/api/abuse/{abuse_id}", post(act))
}

/// 信号类型 → 后台显示的短名。
pub fn kind_label(kind: &str) -> &str {
    match kind {
        "device_exact" => "同设备",
        "device_fuzzy" => "设备相近",
        "fp_missing" => "没交指纹",
        "fp_unknown" => "指纹异常",
        "behavior" => "行为相似",
        "new_account" => "新号",
        "key_sharing" => "共享 Key",
        "same_browser" => "同浏览器",
        "banned_link" => "封禁号关联",
        other => other,
    }
}

/// `"1,2, 3"` → `[1, 2, 3]`（忽略空项和非数字）。
fn key_ids(raw: &str) -> Vec<i64> {
    raw.split(',')
        .filter_map(|s| s.trim().parse::<i64>().ok())
        .collect()
}

/// 一行信号 → 接口返回：detail 解析成对象，附上每把 Key 的名字 / Discord 名。
async fn expand(state: &AppState, mut row: Value) -> Result<Value, ApiError> {
    let raw_detail = row["detail"].as_str().unwrap_or("").to_string();
    let detail: Value = serde_json::from_str(&raw_detail)
        .ok()
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({"summary": raw_detail, "evidence": []}));
    let ids = key_ids(row["key_ids"].as_str().unwrap_or(""));
    let accounts = state.db.abuse_key_accounts(ids).await?;
    let kind = row["kind"].as_str().unwrap_or("").to_string();
    let members = members(
        state,
        row["subject"].as_str().unwrap_or(""),
        &kind,
        &accounts,
    )
    .await?;
    if let Some(obj) = row.as_object_mut() {
        obj.insert("kind_label".into(), json!(kind_label(&kind)));
        obj.insert("detail".into(), detail);
        obj.insert("accounts".into(), Value::Array(accounts));
        obj.insert("members".into(), Value::Array(members));
    }
    Ok(row)
}

/// 信号涉及的每个 Discord 账号（没有 Key 的也列出来）：名字、头像、Key、封禁、账号注册时间、首次 / 最近登录。
/// 同浏览器 / 同设备类信号的登录时间只算共用的那个浏览器 / 设备。不含 IP。
async fn members(
    state: &AppState,
    subject: &str,
    kind: &str,
    accounts: &[Value],
) -> Result<Vec<Value>, ApiError> {
    let ids: Vec<String> = if subject.starts_with("key:") {
        // 共享 Key：subject 是 Key，不是账号；用 Key 的主人
        accounts
            .iter()
            .filter_map(|a| a["discord_id"].as_str())
            .map(str::to_string)
            .collect()
    } else {
        subject
            .split(',')
            .map(str::trim)
            .filter(|d| !d.is_empty())
            .map(str::to_string)
            .collect()
    };
    let scope = match kind {
        "same_browser" => LoginScope::Browser,
        "device_exact" => LoginScope::Device,
        "banned_link" => LoginScope::Both,
        _ => LoginScope::Account,
    };
    let mut rows = state.db.abuse_members(ids, scope).await?;
    for m in &mut rows {
        let id = m["discord_id"].as_str().unwrap_or("").to_string();
        let url = super::members::discord_avatar_url(&id, m["avatar"].as_str());
        if let Some(o) = m.as_object_mut() {
            o.remove("avatar");
            o.insert("avatar_url".into(), json!(url));
            o.insert(
                "created_at".into(),
                json!(gate_core::snowflake::snowflake_created_at(&id)),
            );
        }
    }
    Ok(rows)
}

/// `GET /admin/api/abuse?status=open|dismissed|confirmed|all`（默认 open）
async fn list(State(state): State<AppState>, _s: AdminSession, q: AdminQuery) -> R {
    let status = q.get("status").unwrap_or("open").to_string();
    let filter = match status.as_str() {
        "all" => None,
        s if ABUSE_STATUSES.contains(&s) => Some(s),
        _ => {
            return Err(http(
                422,
                "status 只能是 open / dismissed / confirmed / all",
            ));
        }
    };
    let st = state.clone();
    let rows = st.db.list_abuse_signals(filter, LIST_LIMIT).await?;
    let mut signals = Vec::with_capacity(rows.len());
    for r in rows {
        signals.push(expand(&st, r).await?);
    }
    let open = st.db.count_open_abuse_signals().await?;
    Ok(ok_json(&json!({"signals": signals, "open": open})))
}

/// `POST /admin/api/abuse/{abuse_id}` `{action: dismiss|confirm|reopen}`
async fn act(
    State(state): State<AppState>,
    _s: AdminSession,
    AdminPath(id): AdminPath<i64>,
    req: Request,
) -> R {
    let (parts, body) = json_of(req).await?;
    let st = state.clone();
    let who = actor(&st, &parts);
    let action = body
        .get("action")
        .map(pyval::py_str)
        .unwrap_or_default()
        .to_string();
    let (status, verb) = match action.as_str() {
        "dismiss" => ("dismissed", "忽略待审"),
        "confirm" => ("confirmed", "确认小号"),
        "reopen" => ("open", "重新打开待审"),
        _ => return Err(http(422, "action 只能是 dismiss / confirm / reopen")),
    };
    let Some(row) = st.db.get_abuse_signal(id).await? else {
        return Err(http(404, "这条待审不存在"));
    };
    let kind = row["kind"].as_str().unwrap_or("").to_string();
    let ids = key_ids(row["key_ids"].as_str().unwrap_or(""));
    st.db.set_abuse_status(id, status).await?;

    let accounts = st.db.abuse_key_accounts(ids).await?;
    let names: Vec<String> = accounts
        .iter()
        .map(|a| {
            let name = a["discord_name"]
                .as_str()
                .or_else(|| a["name"].as_str())
                .unwrap_or("?");
            format!("#{} {name}", a["key_id"])
        })
        .collect();
    let mut tagged = 0;
    if action == "confirm" {
        for a in accounts.iter().filter(|a| a["deleted"] != json!(true)) {
            let Some(kid) = a["key_id"].as_i64() else {
                continue;
            };
            st.db
                .upsert_key_tag(kid, SUSPECT_TAG, &kind, "站长", st.now())
                .await?;
            tagged += 1;
        }
    }
    let detail = if action == "confirm" {
        format!(
            "给 {tagged} 把 Key 打「{SUSPECT_TAG}」：{}",
            names.join("、")
        )
    } else {
        names.join("、")
    };
    log_action(
        &st.db,
        &who,
        verb,
        &format!("待审 #{id} {}", kind_label(&kind)),
        gate_core::pyc::take_chars(&detail, 300),
        true,
        st.now(),
    )
    .await;
    let resp = ok_json(&json!({"ok": true, "id": id, "status": status, "tagged": tagged}));
    Ok(mark_logged(resp))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_key_ids_and_labels() {
        assert_eq!(key_ids("3, 1,,x,12"), vec![3, 1, 12]);
        assert_eq!(kind_label("key_sharing"), "共享 Key");
        assert_eq!(kind_label("zzz"), "zzz");
    }
}
