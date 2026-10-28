//! 成员风险线索：筛选与分页、确认、忽略或重新打开。封禁使用独立接口。
//! 确认 = 状态改成 confirmed，并给涉及的每把 Key 打手动标签「风险线索」（note = 信号类型）。不自动封号。

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
use gate_db::repo::abuse::{ABUSE_STATUSES, AbuseFilter, LoginScope, REVIEW_TAG, ReviewAction};
use serde_json::{Value, json};

/// 确认后打在 Key 上的手动标签。
pub const SUSPECT_TAG: &str = REVIEW_TAG;
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
        "ip_switch" => "同网换号",
        "key_concurrent" => "异地同时使用",
        "same_ref_image" => "相同参考图",
        "same_connection" => "外部账号重复",
        "guild_overlap" => "服务器重合",
        "same_battery" => "电量相同",
        "foreign_origin" => "第三方来源",
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
    let member = q.get("member").map(str::to_owned);
    if member
        .as_deref()
        .is_some_and(|id| !gate_core::member_policy::is_discord_id(id))
    {
        return Err(http(422, "成员 ID 格式无效"));
    }
    let parse = |name: &str, default: i64, max: i64| -> Result<i64, ApiError> {
        let n = q
            .get(name)
            .map(|v| v.parse::<i64>())
            .transpose()
            .map_err(|_| http(422, format!("{name} 必须是整数")))?
            .unwrap_or(default);
        if !(1..=max).contains(&n) {
            return Err(http(422, format!("{name} 超出范围")));
        }
        Ok(n)
    };
    let page = parse("page", 1, 10000)?;
    let per = parse("per", LIST_LIMIT, LIST_LIMIT)?;
    let query = AbuseFilter {
        status: filter.map(str::to_owned),
        member: member.clone(),
        limit: per,
        offset: (page - 1) * per,
    };
    let rows = st.db.list_abuse_signals(&query).await?;
    let mut signals = Vec::with_capacity(rows.len());
    for row in rows {
        signals.push(expand(&st, row).await?);
    }
    let counts = st.db.abuse_signal_counts(member.as_deref()).await?;
    let total = match filter {
        Some("open") => counts.open,
        Some("confirmed") => counts.confirmed,
        Some("dismissed") => counts.dismissed,
        _ => counts.total,
    };
    Ok(ok_json(
        &json!({"signals":signals,"open":counts.open,"counts":counts,"total":total,"page":page,"per":per}),
    ))
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
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct ReviewRequest {
        action: ReviewAction,
    }
    let request: ReviewRequest = serde_json::from_value(Value::Object(body))
        .map_err(|_| http(422, "action 只能是 dismiss / confirm / reopen"))?;
    let action = request.action;
    let status = action.status();
    let verb = match action {
        ReviewAction::Dismiss => "忽略待审",
        ReviewAction::Confirm => "确认线索",
        ReviewAction::Reopen => "重新打开待审",
    };
    let Some(result) = st
        .db
        // 标签的 by 与手动打标一致记「站长」（操作日志才细到「后台 IP」）
        .review_abuse_signal(id, action, "站长", st.now())
        .await?
    else {
        return Err(http(404, "这条待审不存在"));
    };
    let kind = result.kind;
    let ids = result.key_ids;
    let tagged = result.tagged;
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
    let detail = if action == ReviewAction::Confirm {
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
        gate_core::primitives::take_chars(&detail, 300),
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
