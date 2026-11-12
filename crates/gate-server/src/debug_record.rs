//! 临时调试记录：按 Discord 成员 ID 把提示词、整理后的参数和生成图落到数据目录。
//!
//! 默认关闭。不进主库、不恢复 `generation_audit`。关掉名单后不再写入；已有文件由站长自行删除。

use crate::error::{ApiResult, HttpError, ok_json};
use crate::fsutil;
use crate::key::Key;
use crate::reqctx::ReqCtx;
use crate::session::AdminSession;
use crate::state::AppState;
use axum::Json;
use axum::Router;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderValue, header};
use axum::response::Response;
use axum::routing::get;
use base64::Engine;
use gate_core::member_policy::is_discord_id;
use gate_core::primitives::{len_chars, take_chars};
use gate_core::values::text;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path as FsPath, PathBuf};

pub const SETTING_KEY: &str = "debug_record_subjects";
const DIR_NAME: &str = "debug_records";
const BINARY_KEYS: &[&str] = &[
    "image",
    "mask",
    "reference_image",
    "reference_image_multiple",
    "director_reference_images",
    "vibe_transfer_image",
];

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/admin/api/ops/debug-record",
            get(get_config).put(put_config),
        )
        .route(
            "/admin/api/ops/debug-record/{subject}/{session}/{file}",
            get(get_file),
        )
}

pub async fn watching(state: &AppState, subject: &str) -> bool {
    !subject.is_empty() && subjects(state).await.iter().any(|id| id == subject)
}

pub async fn capture_request(
    state: &AppState,
    ctx: &ReqCtx,
    key: &Key,
    kind: &str,
    body: &Value,
    notes: &[String],
    detail: &str,
) {
    let subject = key.subject();
    if !watching(state, subject).await {
        return;
    }
    let dir = session_dir(state, subject, &ctx.internal_id());
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::warn!(target: "debug_record", error = %e, "create dir");
        return;
    }
    let mode = std::fs::Permissions::from_mode(0o700);
    let _ = std::fs::set_permissions(&dir, mode.clone());
    if let Some(parent) = dir.parent() {
        let _ = std::fs::set_permissions(parent, mode);
    }
    let sanitized = sanitize(body);
    let meta = json!({
        "ts": crate::time::now(),
        "rid": ctx.rid(),
        "key_id": key.id(),
        "subject": subject,
        "kind": kind,
        "model": text(body.get("model").unwrap_or(&Value::Null)),
        "detail": detail,
        "notes": notes,
        "client": ctx.origin().2,
        "prompt_chars": prompt_chars(body),
    });
    write_json(&dir.join("meta.json"), &meta);
    write_json(&dir.join("request.json"), &sanitized);
    save_input_images(&dir, body);
}

pub async fn capture_images(state: &AppState, ctx: &ReqCtx, subject: &str, images: &[Vec<u8>]) {
    capture_outputs(state, ctx, subject, images, &[]).await;
}

/// 解出的图优先；解不出来时把原始响应体也留下，避免 ZIP/单图提取失败时这次记录空着。
pub async fn capture_outputs(
    state: &AppState,
    ctx: &ReqCtx,
    subject: &str,
    images: &[Vec<u8>],
    raw: &[u8],
) {
    if !watching(state, subject).await {
        return;
    }
    let dir = session_dir(state, subject, &ctx.internal_id());
    if !dir.is_dir()
        && let Err(e) = std::fs::create_dir_all(&dir)
    {
        tracing::warn!(target: "debug_record", error = %e, "create dir");
        return;
    }
    let extracted: Vec<&[u8]> = images
        .iter()
        .map(Vec::as_slice)
        .filter(|b| !b.is_empty())
        .collect();
    let frames: Vec<&[u8]> = if extracted.is_empty() {
        if raw.is_empty() {
            return;
        }
        vec![raw]
    } else {
        extracted
    };
    refresh_rid(&dir, &ctx.rid());
    for (i, bytes) in frames.into_iter().enumerate() {
        write_image(&dir, &format!("out-{i}"), bytes);
    }
}

pub fn write_output_image(
    state: &AppState,
    subject: &str,
    session: &str,
    index: i64,
    bytes: &[u8],
) {
    if subject.is_empty() || bytes.is_empty() {
        return;
    }
    let dir = session_dir(state, subject, session);
    let _ = std::fs::create_dir_all(&dir);
    write_image(&dir, &format!("out-{index}"), bytes);
}

async fn subjects(state: &AppState) -> Vec<String> {
    let raw = state
        .db
        .get_setting(SETTING_KEY)
        .await
        .ok()
        .flatten()
        .unwrap_or_default();
    parse_subjects(&raw)
}

fn parse_subjects(raw: &str) -> Vec<String> {
    let Ok(Value::Array(items)) = serde_json::from_str::<Value>(raw) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|v| v.as_str())
        .filter(|s| is_discord_id(s))
        .map(str::to_string)
        .collect()
}

fn root(state: &AppState) -> PathBuf {
    state
        .db
        .path()
        .parent()
        .unwrap_or_else(|| FsPath::new("."))
        .join(DIR_NAME)
}

fn session_dir(state: &AppState, subject: &str, session: &str) -> PathBuf {
    root(state).join(subject).join(session)
}

fn write_json(path: &FsPath, value: &Value) {
    if let Ok(bytes) = serde_json::to_vec_pretty(value)
        && let Err(e) = fsutil::write_private(path, &bytes)
    {
        tracing::warn!(target: "debug_record", error = %e, path = %path.display(), "write json");
    }
}

fn refresh_rid(dir: &FsPath, rid: &str) {
    let path = dir.join("meta.json");
    let Ok(bytes) = std::fs::read(&path) else {
        return;
    };
    let Ok(mut meta) = serde_json::from_slice::<Value>(&bytes) else {
        return;
    };
    if meta.get("rid").and_then(Value::as_str) == Some(rid) {
        return;
    }
    meta["rid"] = json!(rid);
    write_json(&path, &meta);
}

fn write_image(dir: &FsPath, stem: &str, bytes: &[u8]) {
    if bytes.is_empty() || bytes.len() > 32 * 1024 * 1024 {
        return;
    }
    let ext = match crate::routes::explore::sniff(bytes) {
        Some("image/png") => "png",
        Some("image/jpeg") => "jpg",
        Some("image/webp") => "webp",
        _ => "bin",
    };
    let path = dir.join(format!("{stem}.{ext}"));
    if fsutil::write_private(&path, bytes).is_err()
        && let Err(e) = std::fs::write(&path, bytes)
    {
        tracing::warn!(target: "debug_record", error = %e, "write image");
    }
}

fn save_input_images(dir: &FsPath, body: &Value) {
    let Some(p) = body.get("parameters") else {
        return;
    };
    if let Some(bytes) = decode_image(p.get("image")) {
        write_image(dir, "in-image", &bytes);
    }
    if let Some(bytes) = decode_image(p.get("mask")) {
        write_image(dir, "in-mask", &bytes);
    }
    if let Some(Value::Array(refs)) = p.get("reference_image_multiple") {
        for (i, item) in refs.iter().enumerate() {
            if let Some(bytes) = decode_image(Some(item)) {
                write_image(dir, &format!("in-ref-{i}"), &bytes);
            }
        }
    }
    if let Some(Value::Array(refs)) = p.get("director_reference_images") {
        for (i, item) in refs.iter().enumerate() {
            if let Some(bytes) = decode_image(Some(item)) {
                write_image(dir, &format!("in-vibe-{i}"), &bytes);
            }
        }
    }
}

fn decode_image(v: Option<&Value>) -> Option<Vec<u8>> {
    let s = v.and_then(Value::as_str)?;
    let raw = s.split(',').next_back().unwrap_or(s).trim();
    base64::engine::general_purpose::STANDARD.decode(raw).ok()
}

fn prompt_chars(body: &Value) -> usize {
    len_chars(&text(body.get("input").unwrap_or(&Value::Null)))
}

pub(crate) fn sanitize(value: &Value) -> Value {
    strip(value, "")
}

fn strip(value: &Value, key: &str) -> Value {
    if BINARY_KEYS.iter().any(|k| *k == key) {
        return omit(value);
    }
    match value {
        Value::String(s) if looks_binary(s) => json!({"_omitted": "bytes", "chars": len_chars(s)}),
        Value::Array(items) => Value::Array(items.iter().map(|item| strip(item, key)).collect()),
        Value::Object(map) => {
            let mut out = Map::new();
            for (k, v) in map {
                out.insert(k.clone(), strip(v, k));
            }
            Value::Object(out)
        }
        other => other.clone(),
    }
}

fn omit(value: &Value) -> Value {
    match value {
        Value::Array(items) => json!({"_omitted": "images", "count": items.len()}),
        Value::String(s) => json!({"_omitted": "bytes", "chars": len_chars(s)}),
        Value::Null => Value::Null,
        _ => json!({"_omitted": "present"}),
    }
}

fn looks_binary(s: &str) -> bool {
    let t = s.trim();
    t.len() > 256
        && (t.starts_with("data:image")
            || t.bytes()
                .filter(|b| b.is_ascii_alphanumeric() || *b == b'+' || *b == b'/' || *b == b'=')
                .count()
                * 20
                / 19
                >= t.len())
}

async fn get_config(State(state): State<AppState>, _s: AdminSession) -> ApiResult {
    let subjects = subjects(&state).await;
    Ok(ok_json(&json!({
        "subjects": subjects,
        "items": list_items(&state, 80),
    })))
}

#[derive(Deserialize)]
struct PutBody {
    subjects: Vec<String>,
}

async fn put_config(
    State(state): State<AppState>,
    _s: AdminSession,
    Json(body): Json<PutBody>,
) -> ApiResult {
    if body.subjects.iter().any(|id| !is_discord_id(id)) {
        return Err(HttpError::new(422, "subjects 必须是 Discord 成员 ID").into());
    }
    let encoded = serde_json::to_string(&body.subjects).unwrap_or_else(|_| "[]".into());
    state.db.set_setting(SETTING_KEY, &encoded).await?;
    Ok(ok_json(&json!({"ok": true, "subjects": body.subjects})))
}

async fn get_file(
    State(state): State<AppState>,
    _s: AdminSession,
    Path((subject, session, file)): Path<(String, String, String)>,
) -> Result<Response, crate::error::ApiError> {
    if !safe_id(&subject) || !safe_session(&session) || !safe_file(&file) {
        return Err(HttpError::new(404, "记录不存在").into());
    }
    let path = session_dir(&state, &subject, &session).join(&file);
    let bytes = std::fs::read(&path).map_err(|_| HttpError::new(404, "记录不存在"))?;
    let mime = crate::routes::explore::sniff(&bytes).unwrap_or(if file.ends_with(".json") {
        "application/json"
    } else {
        "application/octet-stream"
    });
    let mut resp = Response::new(Body::from(bytes));
    resp.headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(mime));
    resp.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

fn list_items(state: &AppState, limit: usize) -> Vec<Value> {
    let root = root(state);
    let mut items = Vec::new();
    let Ok(subjects) = std::fs::read_dir(&root) else {
        return items;
    };
    for subject in subjects.flatten() {
        let name = subject.file_name();
        let subject_id = name.to_string_lossy();
        if !safe_id(&subject_id) {
            continue;
        }
        let Ok(sessions) = std::fs::read_dir(subject.path()) else {
            continue;
        };
        for session in sessions.flatten() {
            let session_id = session.file_name().to_string_lossy().into_owned();
            if !safe_session(&session_id) {
                continue;
            }
            let meta_path = session.path().join("meta.json");
            let meta = std::fs::read(&meta_path)
                .ok()
                .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
                .unwrap_or(json!({}));
            let prompt = std::fs::read(session.path().join("request.json"))
                .ok()
                .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
                .map(|v| take_chars(&text(v.get("input").unwrap_or(&Value::Null)), 160).to_string())
                .unwrap_or_default();
            let images: Vec<String> = std::fs::read_dir(session.path())
                .into_iter()
                .flatten()
                .flatten()
                .filter_map(|e| {
                    let n = e.file_name().to_string_lossy().into_owned();
                    n.starts_with("out-").then_some(n)
                })
                .collect();
            items.push(json!({
                "subject": subject_id,
                "session": session_id,
                "rid": meta.get("rid").cloned().unwrap_or(json!(session_id)),
                "ts": meta.get("ts").cloned().unwrap_or(Value::Null),
                "kind": meta.get("kind").cloned().unwrap_or(Value::Null),
                "model": meta.get("model").cloned().unwrap_or(Value::Null),
                "detail": meta.get("detail").cloned().unwrap_or(Value::Null),
                "prompt": prompt,
                "images": images,
            }));
        }
    }
    items.sort_by(|a, b| {
        b["ts"]
            .as_f64()
            .partial_cmp(&a["ts"].as_f64())
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    items.truncate(limit);
    items
}

fn safe_id(s: &str) -> bool {
    is_discord_id(s)
}

fn safe_session(s: &str) -> bool {
    (8..=32).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn safe_file(s: &str) -> bool {
    let Some((stem, ext)) = s.rsplit_once('.') else {
        return false;
    };
    matches!(ext, "json" | "png" | "jpg" | "webp" | "bin")
        && stem.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        && stem.len() <= 40
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_keeps_prompts_and_drops_image_blobs() {
        let body = json!({
            "input": "1girl, owl",
            "model": "nai-diffusion-5-full",
            "parameters": {
                "width": 1344,
                "negative_prompt": "lowres",
                "image": "a".repeat(300),
                "n_samples": 1
            }
        });
        let out = sanitize(&body);
        assert_eq!(out["input"], "1girl, owl");
        assert_eq!(out["parameters"]["width"], 1344);
        assert_eq!(out["parameters"]["image"]["_omitted"], "bytes");
    }

    #[test]
    fn parse_subjects_rejects_junk() {
        assert!(parse_subjects("nope").is_empty());
        assert_eq!(
            parse_subjects(r#"["900000000000000001","x"]"#),
            vec!["900000000000000001"]
        );
    }
}
