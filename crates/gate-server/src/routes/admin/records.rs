//! 后台请求日志、操作日志和全站重置接口。

use super::mark_logged;
use super::req::{AdminQuery, R, http, json_of};
use crate::action_log::log_action;
use crate::error::ApiError;
use crate::error::ok_json;
use crate::session::AdminSession;
use crate::state::AppState;
use axum::Router;
use axum::extract::{Request, State};
use axum::response::Response;
use axum::routing::{get, post};
use gate_core::{features, primitives};
use gate_db::repo::admin_records::ActionFilter;
use gate_db::repo::usage::LogFilter;
use serde_json::{Value, json};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/admin/api/logs", get(logs))
        .route("/admin/api/actions", get(admin_actions))
        .route("/admin/api/ops/reset-all-keys", post(ops_reset_all_keys))
}

/// 页数：至少 1 页。
fn page_count(total: i64, per_page: i64) -> i64 {
    ((total + per_page - 1) / per_page).max(1)
}

/// `max(1, min(int(page), 1_000_000))`
fn clamp_page(page: i64) -> i64 {
    page.clamp(1, 1_000_000)
}

// ----------------------------------------------------------------------------- 请求日志

/// `key_id=1,2,3`：同一成员的多把 Key 一起查（成员详情「请求」按钮）。每项都得是十进制数字，最多 50 把。
fn key_id_list(raw: &str) -> Result<Vec<i64>, ApiError> {
    let parts: Vec<&str> = raw.split(',').map(str::trim).collect();
    if parts.len() > 50
        || parts
            .iter()
            .any(|x| x.is_empty() || x.len() > 18 || !x.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err(http(422, "Key 编号无效"));
    }
    Ok(parts.iter().filter_map(|x| x.parse().ok()).collect())
}

/// 请求日志（每次请求一行）：按成员 / 功能 / 文生图版本 / 状态 / 原因码 / 错误编号筛选，服务端分页。
/// `summary` 是同一组筛选条件下各状态的条数（不受 `status` 自己影响，这样筛了状态也看得到其他状态有多少）。
async fn logs(State(state): State<AppState>, _s: AdminSession, q: AdminQuery) -> R {
    let key_raw = q.text("key_id").to_string();
    let key_id = if key_raw.contains(',') {
        None
    } else {
        q.opt_int("key_id")?
    };
    let page = q.int("page", 1)?;
    let feature = q.get("feature").map(str::to_string);
    let model_version = q.text("model_version").to_string();
    let rid = q.text("rid").to_string();
    let status = primitives::strip(q.text("status")).to_string();
    let reason = primitives::strip(q.text("reason")).to_string();
    let per_page = q.int("per_page", 20)?;
    let per_page: i64 = if matches!(per_page, 20 | 50 | 100) {
        per_page
    } else {
        20
    };
    let page = clamp_page(page);
    let st = state.clone();
    let key_ids = if key_raw.contains(',') {
        key_id_list(&key_raw)?
    } else {
        Vec::new()
    };
    let feature = feature.filter(|f| !f.is_empty());
    if feature
        .as_deref()
        .is_some_and(|f| !features::FEATURES.iter().any(|(n, _)| *n == f))
    {
        return Err(http(422, "未知功能"));
    }
    if !matches!(model_version.as_str(), "" | "v45" | "v5")
        || (!model_version.is_empty() && feature.as_deref() != Some("image"))
    {
        return Err(http(422, "未知文生图版本"));
    }
    if !matches!(status.as_str(), "" | "ok" | "rejected" | "error") {
        return Err(http(422, "未知状态"));
    }
    if reason.len() > 24
        || !reason
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
    {
        return Err(http(422, "未知原因码"));
    }
    let kinds = feature.as_deref().map(|f| {
        features::kinds_for(f)
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>()
    });
    let rid: String = primitives::strip(&rid)
        .to_lowercase()
        .chars()
        .filter(|c| "0123456789abcdef".contains(*c))
        .take(16)
        .collect();
    let mut filter = LogFilter {
        key_id,
        kinds,
        rid,
        model_version,
        key_ids,
        status: String::new(),
        reason,
    };
    let summary = st.db.log_summary(&filter).await?;
    let total = match status.as_str() {
        "ok" => summary.ok,
        "rejected" => summary.rejected,
        "error" => summary.error,
        _ => summary.total,
    };
    filter.status = status;
    let pages = page_count(total, per_page);
    let page = page.min(pages);
    let mut rows = st
        .db
        .list_logs(per_page, (page - 1) * per_page, &filter)
        .await?;
    // 原因短说明由服务端算（前端不再留一份副本）
    for r in &mut rows {
        if let Value::Object(m) = r {
            let text = |k: &str| m.get(k).and_then(Value::as_str).unwrap_or("").to_string();
            let label = gate_core::reasons::reason_label(&text("reason"), &text("detail"));
            m.insert("reason_label".into(), json!(label));
        }
    }
    let reasons: Vec<Value> = gate_core::reasons::LABELS
        .iter()
        .map(|(code, label)| json!({"code": code, "label": label}))
        .collect();
    Ok(ok_json(&json!({
        "logs": json!(rows),
        "page": page,
        "per_page": per_page,
        "total": total,
        "pages": pages,
        "summary": summary,
        "reasons": reasons,
    })))
}

// ----------------------------------------------------------------------------- 操作日志

async fn admin_actions(State(state): State<AppState>, _s: AdminSession, q: AdminQuery) -> R {
    let page = q.int("page", 1)?;
    let actor_q = q.text("actor").to_string();
    let action_q = q.text("action").to_string();
    let st = state.clone();
    let per_page = 30_i64;
    let filter = ActionFilter {
        actor_kind: primitives::strip(&actor_q).to_string(),
        // 后台按分组筛，一组可含多个旧名
        actions: action_q
            .split('|')
            .map(|a| primitives::strip(a).to_string())
            .filter(|a| !a.is_empty())
            .take(40)
            .collect(),
    };
    let total = st.db.count_actions_filtered(&filter).await?;
    let pages = page_count(total, per_page);
    let page = clamp_page(page).min(pages);
    let mut rows = st
        .db
        .list_actions_filtered(&filter, per_page, (page - 1) * per_page)
        .await?;
    // 「手动」（站长 / Claude）还是「自动」（算法 / 定时任务）；成员自己的操作是 member，认不出的是 ""
    for r in &mut rows {
        if let Value::Object(m) = r {
            let k = gate_core::actor::kind(m.get("actor").and_then(Value::as_str).unwrap_or(""));
            m.insert("kind".into(), json!(k.as_str()));
        }
    }
    let kinds = st.db.action_kinds().await?;
    Ok(ok_json(&json!({
        "actions": json!(rows),
        "page": page,
        "per_page": per_page,
        "total": total,
        "pages": pages,
        "action_kinds": kinds,
    })))
}

// ----------------------------------------------------------------------------- 全站重置

/// 【全站杀头重置】失效所有非邀请 Key 并解除 Discord 绑定映射，以便重新开始。
///
/// 带「邀请」标签的 Key 不受影响：保留启用状态、Discord 绑定和全部标签
/// （2026-10-13 站长：重置是为了腾名额给新成员，私邀的朋友要能继续用；
/// 名额按 discord_registrations 计数，所以绑定必须一并保留，否则邀请成员会丢名额也丢绑定）。
async fn ops_reset_all_keys(State(state): State<AppState>, _s: AdminSession, req: Request) -> R {
    let (_, body) = json_of(req).await?;
    if body.get("confirm") != Some(&json!("确认重置")) {
        return Err(http(400, "安全确认未通过：必须传入 confirm='确认重置'"));
    }
    let st = state.clone();
    let disabled = st.db.reset_all_keys().await?;
    log_action(
        &st.db,
        "站长",
        "全站重置 Key",
        "全部 Key",
        &format!("已失效 {disabled} 把 Key 并清空 Discord 绑定"),
        true,
        st.now(),
    )
    .await;
    let message = format!("已失效 {disabled} 把 Key 并解除 Discord 绑定。");
    let resp: Response = ok_json(&json!({
        "ok": true, "disabled_count": disabled,  "message": message,
    }));
    Ok(mark_logged(resp))
}
