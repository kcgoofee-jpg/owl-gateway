//! 后台「日志与记录」接口（用量日志 / 操作日志 / Bug / 生成记录 / 反馈 / 问卷 / 全站重置）。
//! 导出（zip / CSV）的格式固定，导出的文件有人存档，别随手改列名与列序。

use super::req::{AdminPath, AdminQuery, R, bytes_response, http, internal, json_of, py_quote};
use super::{actor, mark_logged};
use crate::action_log::log_action;
use crate::body::read_json_body;
use crate::error::ApiError;
use crate::error::ok_json;
use crate::feedback::{listing, member_label, safe_cell};
use crate::session::{ADMIN_COOKIE, AdminSession, cookie};
use crate::state::AppState;
use axum::Router;
use axum::extract::{Path, Request, State};
use axum::response::Response;
use axum::routing::{get, post, put};
use gate_core::{features, pyc, pyext, pyval};
use gate_db::repo::admin_records::ActionFilter;
use gate_db::repo::usage::LogFilter;
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/admin/api/logs", get(logs))
        .route("/admin/api/actions", get(admin_actions))
        .route("/admin/api/errors", get(errors_list))
        .route("/admin/api/errors/resolve-all", post(errors_resolve_all))
        .route("/admin/api/errors/{sig}/resolve", post(errors_resolve))
        .route("/admin/api/audit", get(audit_list))
        .route("/admin/api/audit/export", get(audit_export))
        .route("/admin/api/audit/purge-images", post(audit_purge_images))
        .route("/admin/api/audit/{audit_id}/thumb", get(audit_thumb))
        .route("/admin/api/audit/{audit_id}/image", get(audit_image))
        .route("/admin/api/feedback", get(feedback_list))
        .route(
            "/admin/api/feedback/{feedback_id}/review",
            put(feedback_review),
        )
        .route("/admin/api/feedback/export", get(feedback_export))
        .route("/admin/api/survey", get(survey_results))
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

/// 生成记录导出里的单个 Key 编号：只认十进制整数（不能带空格、小数点或逗号）；其余当作没有这把 Key。
fn parse_key_id(raw: &str) -> Option<i64> {
    if raw.is_empty() || !raw.chars().all(pyext::is_digit) {
        return None;
    }
    i64::try_from(pyext::py_int_str(raw).ok()?).ok()
}

/// 请求日志（每次请求一行）：按成员 / 功能 / 状态 / 来源 IP / 原因码 / 错误编号筛选，服务端分页。
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
    let rid = q.text("rid").to_string();
    let status = pyc::strip(q.text("status")).to_string();
    let ip = pyc::strip(q.text("ip")).to_string();
    let reason = pyc::strip(q.text("reason")).to_string();
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
    if !matches!(status.as_str(), "" | "ok" | "rejected" | "error") {
        return Err(http(422, "未知状态"));
    }
    if !ip.is_empty() && ip.parse::<std::net::IpAddr>().is_err() {
        return Err(http(422, "IP 格式不对"));
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
    let rid: String = pyc::strip(&rid)
        .to_lowercase()
        .chars()
        .filter(|c| "0123456789abcdef".contains(*c))
        .take(16)
        .collect();
    let mut filter = LogFilter {
        key_id,
        kinds,
        rid,
        key_ids,
        status: String::new(),
        ip,
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
        actor_kind: pyc::strip(&actor_q).to_string(),
        // 后台按分组筛，一组可含多个旧名
        actions: action_q
            .split('|')
            .map(|a| pyc::strip(a).to_string())
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

// ----------------------------------------------------------------------------- Bug 追踪

/// Bug 追踪：按特征归并的错误（未处理的在前）。detail 含堆栈，只在后台显示。
async fn errors_list(State(state): State<AppState>, _s: AdminSession, q: AdminQuery) -> R {
    let all = q.flag("all")?;
    let st = state.clone();
    let mut items = st.db.list_error_events(all, 100).await?;
    let mut ids: Vec<i64> = items
        .iter()
        .filter_map(|r| r.get("last_key").and_then(Value::as_i64))
        .filter(|k| *k != 0)
        .collect();
    ids.sort_unstable();
    ids.dedup();
    let names: HashMap<i64, String> = st.db.key_names_for(&ids).await?.into_iter().collect();
    for r in &mut items {
        let key_name = match r
            .get("last_key")
            .and_then(Value::as_i64)
            .filter(|k| *k != 0)
        {
            // 名字查不到（或空串）就显示「Key #N (已删除)」
            Some(k) => names
                .get(&k)
                .filter(|n| !n.is_empty())
                .cloned()
                .unwrap_or_else(|| format!("Key #{k} (已删除)")),
            None => String::new(),
        };
        if let Some(o) = r.as_object_mut() {
            o.insert("key_name".into(), Value::String(key_name));
        }
    }
    let open = st.db.open_error_count().await?;
    Ok(ok_json(&json!({"errors": items, "open": open})))
}

/// 标记已处理；之后再出现会作为「复发」重新提醒。
async fn errors_resolve(
    State(state): State<AppState>,
    _s: AdminSession,
    AdminPath(sig): AdminPath<String>,
) -> R {
    let sig = pyc::take_chars(&sig, 12).to_string();
    if !state.db.resolve_error_event(&sig, state.now()).await? {
        return Err(http(404, "没有这条错误记录"));
    }
    Ok(ok_json(&json!({"ok": true})))
}

/// 一键标记所有未处理 Bug 为已处理。
async fn errors_resolve_all(State(state): State<AppState>, _s: AdminSession) -> R {
    let count = state.db.resolve_all_error_events(state.now()).await?;
    Ok(ok_json(&json!({"ok": true, "count": count})))
}

// ----------------------------------------------------------------------------- 生成记录

/// 列表里 `prompt` / `negative` 每个字段最多返回的字符数（Unicode 字符，不是字节）。
/// 生成记录单条最大 ~15KB（提示词 + 负面 + extra），默认页 24 条约 78KB；列表页前端一屏只看得到
/// 前几十个字，完整内容走 `?full=1`（详情）或导出。`extra` 不截断：它是 JSON，前端要解析角色提示词。
const AUDIT_LIST_PROMPT_CHARS: usize = 200;

/// 把列表里的大字段截断到 [`AUDIT_LIST_PROMPT_CHARS`] 个字符；有截断时在行对象末尾加
/// `"truncated": true`（没截断的行不加，和旧响应逐字段兼容）。返回是否有截断。
fn trim_audit_prompt(row: &mut Value) -> bool {
    let Value::Object(m) = row else {
        return false;
    };
    let mut truncated = false;
    for field in ["prompt", "negative"] {
        let Some(text) = m.get(field).and_then(Value::as_str) else {
            continue;
        };
        if text.chars().count() <= AUDIT_LIST_PROMPT_CHARS {
            continue;
        }
        let cut: String = text.chars().take(AUDIT_LIST_PROMPT_CHARS).collect();
        m.insert(field.into(), Value::String(cut));
        truncated = true;
    }
    if truncated {
        m.insert("truncated".into(), Value::Bool(true));
    }
    truncated
}

async fn audit_list(State(state): State<AppState>, _s: AdminSession, q: AdminQuery) -> R {
    let key_id = q.get("key_id").map(str::to_string);
    let page = q.int("page", 1)?;
    let per_page = q.int("per_page", 24)?;
    // 列表默认瘦身（截断提示词）；`?full=1` / `?full=true` 拿完整内容（后台详情/复制完整提示词用）
    let full = match q.text("full") {
        "" | "0" | "false" => false,
        "1" | "true" => true,
        _ => return Err(http(422, "full 只能是 true 或 false")),
    };
    let st = state.clone();
    let per_page: i64 = if matches!(per_page, 24 | 48 | 96) {
        per_page
    } else {
        24
    };
    let mut page = clamp_page(page);
    // 查询串里的 Key 编号：单个，或逗号分隔的同一成员的多把 Key；不是数字的段忽略，一个编号都没有就查不到记录。
    let key_ids: Option<Vec<i64>> = match key_id.as_deref() {
        Some(k) => {
            let mut v = Vec::new();
            for x in k
                .split(',')
                .filter(|x| !x.is_empty() && x.chars().all(pyext::is_digit))
            {
                let n = pyext::py_int_str(x).map_err(|e| internal(e.to_string()))?;
                v.push(i64::try_from(n).map_err(|_| http(422, "key_id 超出范围"))?);
            }
            Some(v)
        }
        None => None,
    };
    let fetch = |page: i64| {
        let st = st.clone();
        let key_ids = key_ids.clone();
        async move {
            let offset = (page - 1) * per_page;
            match key_ids {
                // 给了 key_id 却解析不出任何编号：没有记录（不能落到 list_audit_ids 的「空 = 不筛」）
                Some(ids) if ids.is_empty() => Ok((Vec::new(), 0)),
                Some(ids) => st.db.list_audit_ids(per_page, offset, &ids).await,
                None => st.db.list_audit_ids(per_page, offset, &[]).await,
            }
        }
    };
    let (mut rows, mut total) = fetch(page).await?;
    let pages = page_count(total, per_page);
    if page > pages {
        // 页码超出（跳页太大、或保留期清掉了记录）：回到最后一页，和用量日志 / 操作日志一致
        page = pages;
        (rows, total) = fetch(page).await?;
    }
    // 列表默认只回提示词的前 200 字；完整内容走 `?full=1` / 导出
    if !full {
        for row in &mut rows {
            trim_audit_prompt(row);
        }
    }
    // 只有站长自己看：查看不记操作日志（站长 10-22），导出 / 清空这类改动照记
    Ok(ok_json(&json!({
        "items": json!(rows),
        "page": page,
        "per_page": per_page,
        "total": total,
        "pages": pages,
    })))
}

/// 现场缩图约 70ms/张，翻页回来不再重算；约 400×55KB ≈ 22MB。
/// 进程级缓存——按数据库路径区分实例（同进程里并存多个网关时互不串图）。
struct ThumbCache {
    map: HashMap<(PathBuf, i64), Vec<u8>>,
    order: VecDeque<(PathBuf, i64)>,
}

static THUMBS: LazyLock<Mutex<ThumbCache>> = LazyLock::new(|| {
    Mutex::new(ThumbCache {
        map: HashMap::new(),
        order: VecDeque::new(),
    })
});
const THUMB_CACHE_MAX: usize = 400;

impl ThumbCache {
    fn get(&mut self, k: &(PathBuf, i64)) -> Option<Vec<u8>> {
        let v = self.map.get(k)?.clone();
        self.order.retain(|x| x != k);
        self.order.push_back(k.clone());
        Some(v)
    }
    fn put(&mut self, k: (PathBuf, i64), v: Vec<u8>) {
        self.order.retain(|x| *x != k);
        self.order.push_back(k.clone());
        self.map.insert(k, v);
        while self.order.len() > THUMB_CACHE_MAX {
            if let Some(old) = self.order.pop_front() {
                self.map.remove(&old);
            }
        }
    }
    fn clear_db(&mut self, db: &std::path::Path) {
        self.map.retain(|(p, _), _| p != db);
        self.order.retain(|(p, _)| p != db);
    }
}

fn jpeg_cache_headers() -> [(&'static str, String); 1] {
    [("cache-control", "private, max-age=3600".to_string())]
}

/// 画廊用的小图：不再另存缩略图，从原图现场缩小（浏览器缓存 1 小时）；旧记录仍有存好的缩略图就直接用。
async fn audit_thumb(
    State(state): State<AppState>,
    _s: AdminSession,
    AdminPath(audit_id): AdminPath<i64>,
) -> R {
    let st = state.clone();
    let key = (st.db.path().to_path_buf(), audit_id);
    let cached = THUMBS.lock().expect("thumbs").get(&key);
    let mut data = match cached {
        Some(d) => Some(d),
        None => st.db.audit_thumb(audit_id).await?,
    };
    if data.is_none() {
        let (image, _) = st.db.audit_image(audit_id).await?;
        if let Some(image) = image {
            data = tokio::task::spawn_blocking(move || crate::audit::make_thumbnail(&image))
                .await
                .map_err(internal)?;
            if let Some(d) = &data {
                THUMBS.lock().expect("thumbs").put(key, d.clone());
            }
        }
    }
    let Some(data) = data else {
        return Err(http(404, "没有图片（原图已过期或未记录）"));
    };
    Ok(bytes_response(data, "image/jpeg", &jpeg_cache_headers()))
}

/// 原图（清晰版）。没有原图时回退到缩略图。
async fn audit_image(
    State(state): State<AppState>,
    _s: AdminSession,
    AdminPath(audit_id): AdminPath<i64>,
) -> R {
    let st = state.clone();
    let (mut data, mut ctype) = st.db.audit_image(audit_id).await?;
    if data.is_none() {
        data = st.db.audit_thumb(audit_id).await?;
        ctype = "image/jpeg".into();
    }
    let Some(data) = data else {
        return Err(http(404, "没有图片"));
    };
    let ctype = if ctype.is_empty() {
        "image/png".to_string()
    } else {
        ctype
    };
    Ok(bytes_response(data, &ctype, &jpeg_cache_headers()))
}

/// 导出文件名的净化：只留字母数字 `_.-`，其余字符换成 `_`，去掉两头的 `_.`，最长 40 个字符，空了叫 member。
fn safe_filename(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches(|c| c == '_' || c == '.');
    let cut = pyc::take_chars(trimmed, 40);
    if cut.is_empty() {
        "member".into()
    } else {
        cut.to_string()
    }
}

/// 把某成员的全部原图打包成 zip 下载（附 prompts.txt）。
async fn audit_export(State(state): State<AppState>, _s: AdminSession, q: AdminQuery) -> R {
    let key_id = q.get("key_id").map(str::to_string);
    let Some(key_id) = key_id else {
        return Err(http(400, "缺少 key_id"));
    };
    let st = state.clone();

    // 多个 key_id（逗号分隔，多个成员 / 合并成员的多把 Key）：每个账号各查一遍，合并成一个按账号分文件夹的 zip。
    let ids: Vec<String> = key_id
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if ids.len() > 1 {
        const TOTAL_BUDGET: i64 = 300 * 1024 * 1024; // 原图合计上限，避免一次吃太多内存；提示词不受限
        let mut remaining = TOTAL_BUDGET;
        let mut groups: Vec<crate::exporter::ExportGroup> = Vec::new();
        let mut total_imgs = 0usize;
        for id in &ids {
            // 每个选中的账号都进 zip：原图过期的也把提示词列出来
            // 不是整数的编号当作没有记录
            let Some(key) = parse_key_id(id) else {
                continue;
            };
            let records = st.db.audit_records_for_key(key).await?;
            if records.is_empty() {
                continue;
            }
            let with_image = records.iter().filter(|r| r.has_image).count();
            let images = if remaining > 0 && with_image > 0 {
                st.db.audit_images_for_key(key, remaining, 0.0).await?
            } else {
                Vec::new()
            };
            remaining -= images.iter().map(|r| r.image.len() as i64).sum::<i64>();
            let name = st
                .db
                .key_name(key)
                .await?
                .filter(|n| !n.is_empty())
                .unwrap_or_default();
            let label = if name.is_empty() {
                format!("Key{id}")
            } else {
                format!("{name}-{id}")
            };
            total_imgs += images.len();
            groups.push(crate::exporter::ExportGroup {
                label,
                images_truncated: images.len() < with_image,
                records,
                images,
            });
        }
        if groups.is_empty() {
            return Err(http(404, "选中的成员没有生成记录"));
        }
        let ngroups = groups.len();
        let st2 = st.clone();
        let blob = tokio::task::spawn_blocking(move || {
            crate::exporter::build_zip_groups(&groups, &st2.clock, st2.now())
        })
        .await
        .map_err(internal)?;
        log_action(
            &st.db,
            "站长",
            "导出原图",
            &key_id,
            &format!("{ngroups} 人 / {total_imgs} 张"),
            true,
            st.now(),
        )
        .await;
        let enc = py_quote(&format!("owl-选中{ngroups}人-{total_imgs}.zip"));
        return Ok(bytes_response(
            blob,
            "application/zip",
            &[
                (
                    "content-disposition",
                    format!(
                        "attachment; filename=\"owl-selected-{ngroups}.zip\"; filename*=UTF-8''{enc}"
                    ),
                ),
                ("cache-control", "no-store".to_string()),
            ],
        ));
    }

    // 单成员走这条；`key_id` 不是整数（含 `3,` 这种带逗号的残片）就当没有原图
    let Some(key) = parse_key_id(&key_id) else {
        return Err(http(404, "该成员没有可导出的原图"));
    };
    let rows = st
        .db
        .audit_images_for_key(key, 200 * 1024 * 1024, 0.0)
        .await?;
    if rows.is_empty() {
        return Err(http(404, "该成员没有可导出的原图"));
    }
    let who = st
        .db
        .key_name(key)
        .await?
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| key_id.clone());
    let n = rows.len();
    let st2 = st.clone();
    let who2 = who.clone();
    let blob = tokio::task::spawn_blocking(move || {
        crate::exporter::build_zip(&rows, &who2, &st2.clock, st2.now())
    })
    .await
    .map_err(internal)?;
    log_action(
        &st.db,
        "站长",
        "导出原图",
        &key_id,
        &format!("{n} 张"),
        true,
        st.now(),
    )
    .await;
    let fname = format!("owl-{}-{n}.zip", safe_filename(&who));
    let enc = py_quote(&format!("owl-{who}-{n}.zip"));
    Ok(bytes_response(
        blob,
        "application/zip",
        &[
            (
                "content-disposition",
                format!("attachment; filename=\"{fname}\"; filename*=UTF-8''{enc}"),
            ),
            ("cache-control", "no-store".to_string()),
        ],
    ))
}

/// 站长手动清空服务器上的全部原图（只删原图，提示词和参数照留）。必须带 `{"confirm": "清空"}`。
async fn audit_purge_images(State(state): State<AppState>, _s: AdminSession, req: Request) -> R {
    let (_, body) = json_of(req).await?;
    if body.get("confirm") != Some(&json!("清空")) {
        return Err(http(422, "需要 confirm=清空"));
    }
    let st = state.clone();
    let n = st.db.purge_audit_images_before_id(1_i64 << 62).await?;
    THUMBS.lock().expect("thumbs").clear_db(st.db.path());
    log_action(
        &st.db,
        "站长",
        "清空原图",
        "",
        &format!("{n} 张"),
        true,
        st.now(),
    )
    .await;
    let resp = ok_json(&json!({"ok": true, "purged": n}));
    Ok(mark_logged(resp))
}

// ----------------------------------------------------------------------------- CSV

/// Python `csv.writer`（QUOTE_MINIMAL，行尾 `\r\n`）。
fn csv_line(cells: &[String]) -> String {
    let mut out = String::new();
    for (i, c) in cells.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        if c.contains([',', '"', '\r', '\n']) {
            out.push('"');
            out.push_str(&c.replace('"', "\"\""));
            out.push('"');
        } else {
            out.push_str(c);
        }
    }
    out.push_str("\r\n");
    out
}

/// 带 BOM 的 CSV（Excel 打开不乱码）：表头一行 + 数据行。
fn csv_bytes(header: &[&str], rows: &[Vec<String>]) -> Vec<u8> {
    let mut s = String::from('\u{feff}');
    s.push_str(&csv_line(
        &header.iter().map(|h| (*h).to_string()).collect::<Vec<_>>(),
    ));
    for r in rows {
        s.push_str(&csv_line(r));
    }
    s.into_bytes()
}

/// 成员 /反馈 写的话，最新在前。每条的 `text` 是显示用的文字（2.x 问卷的旧记录是一行摘要，`legacy` 为真）。
async fn feedback_list(State(state): State<AppState>, _s: AdminSession, req: Request) -> R {
    let parts = req.into_parts().0;
    let items = listing(&state.db, 500).await?;
    let csrf_token = super::ops::scoped_csrf(
        &state,
        "feedback-review",
        &cookie(&parts.headers, ADMIN_COOKIE).unwrap_or_default(),
    );
    Ok(ok_json(&json!({"items": items, "csrf_token": csrf_token})))
}

async fn feedback_review(
    State(state): State<AppState>,
    Path(feedback_id): Path<i64>,
    _s: AdminSession,
    req: Request,
) -> R {
    let (parts, body) = req.into_parts();
    if !super::ops::origin_ok(&parts.headers) {
        return Err(http(403, "请求来源校验失败"));
    }
    let expected = super::ops::scoped_csrf(
        &state,
        "feedback-review",
        &cookie(&parts.headers, ADMIN_COOKIE).unwrap_or_default(),
    );
    let given = crate::auth::header_str(&parts.headers, "x-nai-admin-csrf");
    if !crate::session::ct_eq(given.as_bytes(), expected.as_bytes()) {
        return Err(http(403, "会话校验失败，请刷新后台后重试"));
    }
    let value = read_json_body(&parts.headers, body, 1024).await?;
    let Some(reviewed) = value.get("reviewed").and_then(Value::as_bool) else {
        return Err(http(422, "审核状态必须是布尔值"));
    };
    let who = actor(&state, &parts);
    let changed = state
        .db
        .set_feedback_reviewed(
            feedback_id,
            reviewed.then_some(state.now()),
            if reviewed { &who } else { "" },
        )
        .await?;
    if !changed {
        return Err(http(404, "反馈不存在"));
    }
    log_action(
        &state.db,
        &who,
        if reviewed {
            "查看成员反馈"
        } else {
            "重新打开成员反馈"
        },
        &format!("反馈 #{feedback_id}"),
        "",
        true,
        state.now(),
    )
    .await;
    Ok(ok_json(&json!({"id": feedback_id, "reviewed": reviewed})))
}

/// CSV 三列：time、member（Key 名 / 用户名 / Discord ID）、text（2.x 问卷的旧记录是同一行摘要）。
async fn feedback_export(State(state): State<AppState>, _s: AdminSession, req: Request) -> R {
    let parts = req.into_parts().0;
    let st = state.clone();
    let mut items = listing(&st.db, 100_000).await?;
    items.sort_by(|a, b| {
        pyval::number_like(&a["ts"])
            .partial_cmp(&pyval::number_like(&b["ts"]))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let rows: Vec<Vec<String>> = items
        .iter()
        .map(|it| {
            vec![
                st.clock
                    .format(it["ts"].as_f64().unwrap_or(0.0), "%Y-%m-%d %H:%M:%S"),
                safe_cell(&member_label(it)),
                safe_cell(it["text"].as_str().unwrap_or("")),
            ]
        })
        .collect();
    let text = csv_bytes(&["time", "member", "text"], &rows);
    log_action(
        &st.db,
        &actor(&st, &parts),
        "导出成员反馈",
        "全部CSV",
        "",
        true,
        st.now(),
    )
    .await;
    let fn_ = format!(
        "owl-feedback-{}.csv",
        st.clock.format(st.now(), "%Y%m%d-%H%M")
    );
    Ok(bytes_response(
        text,
        "text/csv; charset=utf-8",
        &[
            (
                "content-disposition",
                format!(
                    "attachment; filename=\"{fn_}\"; filename*=UTF-8''{}",
                    py_quote(&fn_)
                ),
            ),
            ("cache-control", "no-store".to_string()),
        ],
    ))
}

// ----------------------------------------------------------------------------- 问卷

/// 成员详细问卷（/survey 网页页）：逐条明细（选项汇总由前端按明细算）；Discord /反馈 的结果在 /feedback。
async fn survey_results(State(state): State<AppState>, _s: AdminSession) -> R {
    let rows = state.db.list_survey().await?;
    Ok(ok_json(&json!({"total": rows.len(), "rows": rows})))
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
    let (disabled, kept) = st.db.reset_all_keys().await?;
    log_action(
        &st.db,
        "站长",
        "全站重置 Key",
        "全部非邀请 Key",
        &format!(
            "已失效 {disabled} 把 Key 并清空 Discord 绑定；「邀请」Key 保留 {kept} 把不受影响"
        ),
        true,
        st.now(),
    )
    .await;
    let message = format!(
        "全站已成功重置！已失效 {disabled} 把 Key 并解除 Discord 绑定{}",
        if kept > 0 {
            format!("；{kept} 把「邀请」Key 原样保留。")
        } else {
            "。".to_string()
        }
    );
    let resp: Response = ok_json(&json!({
        "ok": true, "disabled_count": disabled, "kept_invites": kept, "message": message,
    }));
    Ok(mark_logged(resp))
}
