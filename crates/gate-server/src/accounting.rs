//! 记账与成员接口的错误收尾。
//!
//! - `record`：写用量日志；成功请求同时计入每日配额。写库在后台任务里做（关停时等它们写完）。
//! - `settle_record`：成功出图的那一笔必须在额度预留 / 并发槽释放之前落库。后台任务不随调用方取消，
//!   所以「等它写完」是天经地义的语义，记账路径可以放心 `.await`。

use crate::bugs::Report;
use crate::error::{GateErrorInfo, InternalErrorInfo};
use crate::key::Key;
use crate::reqctx::ReqCtx;
use crate::state::AppState;
use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::Response;
use gate_db::repo::usage::{NewLog, Success};

/// 一条用量记录。
///
/// `legacy_free_images` 的 legacy 指 NovelAI 的「V4.5 及更早模型」（与 V5 相对，不是遗留代码）：
/// 记的是这类模型当天出了几张，和每日 V4.5 额度对应。
#[derive(Debug, Clone, Default)]
pub struct Entry {
    pub kind: String,
    pub model: String,
    pub status: String,
    pub images: i64,
    pub anlas: f64,
    pub tokens: i64,
    pub v5: i64,
    pub legacy_free_images: i64,
    pub detail: String,
    pub unconfirmed_anlas: f64,
    pub reason: String,
    /// see `gate_db::repo::usage::Success::site_paid_anlas`
    pub site_paid_anlas: f64,
    pub execution_unknown: bool,
    pub remaining_anlas: f64,
    pub remaining_v5: i64,
    pub remaining_legacy: i64,
}

impl Entry {
    pub fn new(kind: &str, model: &str, status: &str) -> Self {
        Self {
            kind: kind.into(),
            model: model.into(),
            status: status.into(),
            ..Default::default()
        }
    }
    pub fn detail(mut self, d: impl Into<String>) -> Self {
        self.detail = d.into();
        self
    }
    pub fn reason(mut self, r: impl Into<String>) -> Self {
        self.reason = r.into();
        self
    }
    pub fn unconfirmed(mut self, a: f64) -> Self {
        self.unconfirmed_anlas = a;
        self
    }
    pub fn execution_unknown(
        mut self,
        unknown: bool,
        remaining_anlas: f64,
        remaining_v5: i64,
        remaining_legacy: i64,
    ) -> Self {
        self.execution_unknown = unknown;
        self.remaining_anlas = remaining_anlas;
        self.remaining_v5 = remaining_v5;
        self.remaining_legacy = remaining_legacy;
        self
    }
}

/// 路径 → 用量日志里的 kind（按顺序子串匹配）。
const PATH_KIND: [(&str, &str); 6] = [
    ("generate-image-stream", "image_stream"),
    ("suggest-tags", "tags"),
    ("generate-image", "image"),
    ("encode-vibe", "vibe_encode"),
    ("upscale", "upscale"),
    ("augment-image", "augment-image"),
];

pub fn kind_for_path(path: &str) -> &'static str {
    if path == "/" {
        return "image"; // 根地址 POST 也是出图（见 routes::image::router）
    }
    PATH_KIND
        .iter()
        .find(|(needle, _)| path.contains(needle))
        .map_or("account", |(_, k)| k)
}

impl AppState {
    /// 这个请求的 `(User-Agent, 来源 IP)`：IP 按 `FORWARDED_ALLOW_IPS` 信任列表从 X-Forwarded-For 里取真实客户端地址。
    pub fn origin_of(&self, ctx: &ReqCtx) -> (String, String) {
        let (peer, xff, ua) = ctx.origin();
        let peer = (!peer.is_empty()).then_some(peer);
        let ip = crate::auth::client_ip_from(peer.as_deref(), &xff, &self.env.forwarded_allow_ips);
        (ua, if ip == "unknown" { String::new() } else { ip })
    }

    /// 记一个 Bug（自动带上当前请求的编号和 Key）。
    pub fn bug(&self, source: &str, error: Option<(&str, &str)>, ctx: &ReqCtx) {
        self.bugs.capture(Report {
            source,
            error,
            rid: &ctx.rid(),
            key_id: ctx.key().map(|(id, _)| id),
            ..Default::default()
        })
    }

    /// 写一条用量日志（后台任务）；成功请求额外计入每日配额。返回任务句柄。
    pub fn record(&self, ctx: &ReqCtx, key: &Key, mut e: Entry) -> tokio::task::JoinHandle<()> {
        ctx.mark_logged();
        // 万一有拒绝路径没带原因码：按说明文字当场归类（只管 status='rejected'）。
        if e.status == "rejected" && e.reason.is_empty() {
            e.reason = gate_core::reasons::classify(&e.detail).to_string();
        }
        self.live_note(&e.status);
        let t = ctx.snapshot();
        let (client, ip) = self.origin_of(ctx);
        let hints = ctx.hints();
        let (key_id, key_name) = (key.id(), key.name().to_string());
        let state = self.clone();
        let ctx = ctx.clone();
        self.tasks.spawn(async move {
            let now = state.now();
            let settlement_reserved = ctx.has_settlement_reservation();
            let settlement_uncertain = e.execution_unknown || ctx.execution_unknown();
            let res = if e.status == "ok" {
                let success = Success {
                    key_id,
                    key_name,
                    kind: e.kind.clone(),
                    model: e.model,
                    day: ctx.settlement_day().unwrap_or_else(|| state.day()),
                    images: e.images,
                    anlas: e.anlas,
                    tokens: e.tokens,
                    v5: e.v5,
                    legacy_free_images: e.legacy_free_images,
                    detail: e.detail,
                    unconfirmed_anlas: e.unconfirmed_anlas,
                    remaining_anlas: e.remaining_anlas,
                    remaining_v5: e.remaining_v5,
                    remaining_legacy: e.remaining_legacy,
                    wait_ms: t.wait_ms,
                    dur_ms: t.dur_ms,
                    up_status: i64::from(t.up_status),
                    rid: t.rid,
                    token_id: t.token_id,
                    client,
                    ip,
                    hints,
                    site_paid_anlas: e.site_paid_anlas,
                };
                if ctx.has_settlement_reservation() {
                    state
                        .db
                        .record_success_settled(&success, &ctx.internal_id(), now)
                        .await
                } else {
                    state.db.record_success(&success, now).await
                }
            } else {
                let failure = NewLog {
                    key_id: Some(key_id),
                    key_name,
                    kind: e.kind.clone(),
                    model: e.model,
                    status: e.status.clone(),
                    images: e.images,
                    anlas: e.anlas,
                    tokens: e.tokens,
                    detail: e.detail,
                    unconfirmed_anlas: e.unconfirmed_anlas,
                    wait_ms: t.wait_ms,
                    dur_ms: t.dur_ms,
                    up_status: i64::from(t.up_status),
                    rid: t.rid,
                    reason: e.reason,
                    token_id: t.token_id,
                    client,
                    ip,
                    hints,
                };
                if settlement_reserved {
                    state
                        .db
                        .record_settled_failure(
                            &failure,
                            &ctx.internal_id(),
                            settlement_uncertain,
                            now,
                        )
                        .await
                } else {
                    state.db.add_log(&failure, now).await
                }
            };
            if let Err(err) = res {
                tracing::error!("request accounting failed");
                // 「成员拿到了图但账没落库」的唯一线索：带上定位信息
                state.bugs.capture(Report {
                    source: "accounting",
                    error: Some(("DbError", &err.to_string())),
                    detail: &format!(
                        "key_id={key_id} kind={} status={} images={}",
                        e.kind, e.status, e.images
                    ),
                    rid: &ctx.rid(),
                    key_id: Some(key_id),
                    ..Default::default()
                });
            }
        })
    }

    /// 成功的那笔账在释放额度预留 / 并发槽之前写完；调用方被取消也不会中断写入。
    pub async fn settle_record(&self, ctx: &ReqCtx, key: &Key, e: Entry) {
        let _ = self.record(ctx, key, e).await;
    }
}

/// 成员接口错误的收尾（放在请求上下文中间件里面）：
/// - 鉴权之后被拒（4xx）且本请求还没记过日志：统一记一条「rejected」，方便排查成员问题；
/// - 5xx 的 GateError：按消息归并成 Bug（网关自己的故障）；原因在上游的（`GateError::upstream`）不记——
///   上游故障在用量日志和「上游」页里看得到，不是本站的缺陷；
/// - 未预料的内部错误：记 Bug，有 Key 且没记过日志时补一条「error」；上游传输层失败
///   （`error::upstream_transport`）对成员仍是 500，但同样不记 Bug。
pub async fn error_ledger(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let path = req.uri().path().to_string();
    let ctx = req.extensions().get::<ReqCtx>().cloned();
    let resp = next.run(req).await;
    let Some(ctx) = ctx else { return resp };
    if let Some(GateErrorInfo(e)) = resp.extensions().get::<GateErrorInfo>() {
        if let Some((key_id, key_name)) = ctx.key()
            && (400..500).contains(&e.status)
            && !ctx.logged()
        {
            let key = Key(serde_json::json!({"id": key_id, "name": key_name}));
            let detail =
                gate_core::primitives::take_chars(&format!("{} {}", e.status, e.message), 160)
                    .to_string();
            state.record(
                &ctx,
                &key,
                Entry::new(kind_for_path(&path), &ctx.model(), "rejected")
                    .detail(detail)
                    .reason(e.code.clone()),
            );
        }
        if e.status >= 500 && !e.upstream {
            let title =
                gate_core::primitives::take_chars(&format!("{} {}", e.status, e.message), 200)
                    .to_string();
            state.bugs.capture(Report {
                source: if matches!(e.status, 502..=504) {
                    "upstream"
                } else {
                    "gate"
                },
                title: &title,
                path: &path,
                level: "warn",
                rid: &ctx.rid(),
                key_id: ctx.key().map(|(id, _)| id),
                ..Default::default()
            });
        }
    } else if let Some(InternalErrorInfo(msg)) = resp.extensions().get::<InternalErrorInfo>() {
        if !ctx.upstream_fault() {
            state.bugs.capture(Report {
                source: "request",
                error: Some(("InternalError", msg)),
                path: &path,
                rid: &ctx.rid(),
                key_id: ctx.key().map(|(id, _)| id),
                ..Default::default()
            });
        }
        if let Some((key_id, key_name)) = ctx.key()
            && !ctx.logged()
        {
            let t = ctx.snapshot();
            let (client, ip) = state.origin_of(&ctx);
            let hints = ctx.hints();
            let db = state.db.clone();
            let now = state.now();
            let kind = kind_for_path(&path).to_string();
            state.tasks.spawn(async move {
                let _ = db
                    .add_log(
                        &NewLog {
                            key_id: Some(key_id),
                            key_name,
                            kind,
                            status: "error".into(),
                            detail: "500 InternalError".into(),
                            rid: t.rid,
                            token_id: t.token_id,
                            client,
                            ip,
                            hints,
                            ..Default::default()
                        },
                        now,
                    )
                    .await;
            });
        }
    }
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_kinds() {
        assert_eq!(kind_for_path("/ai/generate-image-stream"), "image_stream");
        assert_eq!(kind_for_path("/nai/ai/generate-image"), "image");
        assert_eq!(kind_for_path("/ai/generate-image/suggest-tags"), "tags");
        // 文本接口已删除，这些路径不会再走到记账；按「账户查询」兜底
        assert_eq!(kind_for_path("/ai/generate"), "account");
        assert_eq!(kind_for_path("/user/subscription"), "account");
        assert_eq!(kind_for_path("/"), "image");
    }
}
