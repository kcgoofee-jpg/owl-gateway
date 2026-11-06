//! 工作台自动备份：成员主动开启后，把本站生图页成功生成的图片转发到其个人 Discord 频道。
//!
//! 原则（与产品决定一致）：
//! - 只认服务端**已完整收到**的最终图片：非流式从已验证的响应体解包，流式从 `final` 事件解包；
//!   预览图、参考图、导入图、历史恢复图都不会走到这里；
//! - 投递完全在服务端：页面关闭不影响已接收的图片；重启丢内存队列，不自动重发；
//! - 图片只在内存队列里短暂保存（条数 + 字节上限），不落盘、不进数据库；
//!   备份状态只记请求号 / 图片序号 / 结果，不含图片字节与提示词；
//! - 限流按 Discord 的 Retry-After 等待；结果未知不重发（避免重复消息，从浏览器历史手动补传）；
//!   401/403/404 视为频道失效，停止该成员的后续尝试并提示重连，绝不回退 OWL 公共频道；
//! - 备份与计费账本隔离：这里不写 usage_log / request_settlements，也不重放上游生成。

use crate::discord_share;
use crate::error::ApiError;
use crate::state::AppState;
use serde_json::json;
use std::collections::{HashMap, VecDeque};

/// 单张图片字节上限（对齐 explore 的 32MB）。
const MAX_IMAGE_BYTES: usize = 32 * 1024 * 1024;
/// 单成员队列里最多积压几张图（多图请求一次性入队，超出的直接丢弃）。
const MAX_QUEUE_PER_MEMBER: usize = 8;
/// 单成员待投递字节上限：慢速限流时不让内存被一位成员拖爆。
const MAX_MEMBER_QUEUE_BYTES: usize = 96 * 1024 * 1024;
/// 同时投递的 worker 数。
const WORKERS: usize = 4;
/// 设置键：成员级开关（"1" = 开）。图片或提示词永不进 settings。
pub const BACKUP_ENABLED_PREFIX: &str = "discord_backup:";

struct BackupJob {
    member_id: String,
    request_id: String,
    image_index: i64,
    bytes: Vec<u8>,
}

impl BackupJob {
    /// 实际附件格式（按文件头认），决定扩展名与 MIME。
    fn sniff(&self) -> (&'static str, &'static str) {
        match crate::routes::explore::sniff(&self.bytes) {
            Some("image/png") => ("image/png", "png"),
            Some("image/jpeg") => ("image/jpeg", "jpg"),
            Some("image/webp") => ("image/webp", "webp"),
            _ => ("application/octet-stream", "bin"),
        }
    }
}

/// 成员的内存队列 + 失效标记。
#[derive(Default)]
struct MemberQueue {
    jobs: VecDeque<BackupJob>,
    bytes: usize,
    /// Discord 明确 401/403/404 后置位：worker 丢弃该成员的积压，直到成员重新连接并重开备份。
    dead: bool,
    /// 该成员限流到这个时间点（进程内，重启即清）。
    cooldown_until: f64,
}

/// 自动备份中心：生成路径只管 push，worker 慢慢投。
#[derive(Default)]
pub struct BackupCenter {
    queues: std::sync::Mutex<HashMap<String, MemberQueue>>,
    changed: tokio::sync::Notify,
}

impl BackupCenter {
    /// 服务端收到完整图片后调用。只留在内存，等 worker 投递。
    pub fn enqueue(&self, member_id: &str, request_id: &str, image_index: i64, bytes: Vec<u8>) {
        if bytes.is_empty() || bytes.len() > MAX_IMAGE_BYTES {
            return;
        }
        let mut queues = self.queues.lock().expect("backup queues");
        let q = queues.entry(member_id.to_string()).or_default();
        if q.dead
            || q.jobs.len() >= MAX_QUEUE_PER_MEMBER
            || q.bytes + bytes.len() > MAX_MEMBER_QUEUE_BYTES
        {
            tracing::warn!(
                target: "backup",
                member = %member_id,
                request_id = %request_id,
                image_index,
                "auto-backup queue full; image dropped (restore manually from browser history)"
            );
            return;
        }
        q.bytes += bytes.len();
        q.jobs.push_back(BackupJob {
            member_id: member_id.to_string(),
            request_id: request_id.to_string(),
            image_index,
            bytes,
        });
        drop(queues);
        self.changed.notify_one();
    }

    /// 取一条要投递的作业（克隆队首；不弹出——结果确认后再 pop）。
    /// 所有成员都在冷却 / 队列空时返回 None。
    fn next(&self) -> Option<BackupJob> {
        let mut queues = self.queues.lock().expect("backup queues");
        let now = crate::time::now();
        let mut ready: Option<(String, f64)> = None;
        queues.retain(|_, q| !(q.dead && q.jobs.is_empty()));
        for (member, q) in queues.iter() {
            if q.dead || q.jobs.is_empty() {
                continue;
            }
            if q.cooldown_until > now {
                continue;
            }
            // 平衡各成员：取冷却结束最早的。
            if ready.as_ref().is_none_or(|(_, t)| q.cooldown_until < *t) {
                ready = Some((member.clone(), q.cooldown_until));
            }
        }
        let (member, _) = ready?;
        let q = queues.get_mut(&member)?;
        let job = q.jobs.front()?;
        Some(BackupJob {
            member_id: job.member_id.clone(),
            request_id: job.request_id.clone(),
            image_index: job.image_index,
            bytes: job.bytes.clone(),
        })
    }

    fn cooldown(&self, member_id: &str, seconds: f64) {
        if let Some(q) = self
            .queues
            .lock()
            .expect("backup queues")
            .get_mut(member_id)
        {
            q.cooldown_until = crate::time::now() + seconds;
        }
    }

    /// 频道失效：清空该成员积压并停用（成员重连后需重新开启自动备份）。
    fn mark_dead(&self, member_id: &str) {
        let mut queues = self.queues.lock().expect("backup queues");
        if let Some(q) = queues.get_mut(member_id) {
            q.dead = true;
            q.jobs.clear();
            q.bytes = 0;
        }
    }

    /// 投递完成（成功 / 放弃）：弹出队首并释放字节。
    fn pop(&self, member_id: &str) {
        let mut queues = self.queues.lock().expect("backup queues");
        if let Some(q) = queues.get_mut(member_id)
            && let Some(job) = q.jobs.pop_front()
        {
            q.bytes = q.bytes.saturating_sub(job.bytes.len());
        }
    }
}

/// 成员是否开了自动备份。
pub async fn enabled(state: &AppState, member_id: &str) -> bool {
    state
        .db
        .get_setting(&format!("{BACKUP_ENABLED_PREFIX}{member_id}"))
        .await
        .ok()
        .flatten()
        .is_some_and(|v| v == "1")
}

/// 开启 / 关闭自动备份。开启时要求已有个人频道绑定。
pub async fn set_enabled(state: &AppState, member_id: &str, on: bool) -> Result<(), ApiError> {
    if on
        && discord_share::destination(state, member_id)
            .await?
            .is_none()
    {
        return Err(crate::error::HttpError::new(409, "请先连接个人频道，再开启自动备份。").into());
    }
    state
        .db
        .set_setting(
            &format!("{BACKUP_ENABLED_PREFIX}{member_id}"),
            if on { "1" } else { "" },
        )
        .await?;
    Ok(())
}

/// 这条请求是否参与自动备份：只有本站生图页（bridge 的 workbench 标记）+ 成员 Key。
/// 第三方客户端、插件一律不参与（插件自动备份先调研，不纳入本次实现）。
pub fn applies_to(client_ua: &str, member_id: &str) -> bool {
    !member_id.is_empty() && client_ua.starts_with("workbench")
}

/// 生成成功后触发：逐张入队（异步确认开关与目的地，避免同步阻塞出图路径）。
/// request_id 是投递去重与日志关联的主键；image_index 区分多图。
pub async fn on_generation_ok(
    state: &AppState,
    ctx: &crate::reqctx::ReqCtx,
    member_id: &str,
    images: Vec<Vec<u8>>,
) {
    if images.is_empty() || !applies_to(&ctx.origin().2, member_id) {
        return;
    }
    if !enabled(state, member_id).await {
        return;
    }
    let request_id = ctx.internal_id();
    for (index, bytes) in images.into_iter().enumerate() {
        state
            .backup
            .enqueue(member_id, &request_id, index as i64, bytes);
    }
}

/// 启动投递 worker：随进程生命周期，关停时随 TaskTracker 一起结束。
pub fn spawn_worker(state: AppState) {
    for _ in 0..WORKERS {
        let s = state.clone();
        state.tasks.spawn(async move {
            loop {
                if s.shutdown.is_cancelled() {
                    break;
                }
                let Some(job) = s.backup.next() else {
                    tokio::select! {
                        _ = s.shutdown.cancelled() => break,
                        _ = s.backup.changed.notified() => {}
                    }
                    continue;
                };
                deliver_one(&s, job).await;
            }
        });
    }
}

/// 投递一条作业：确认开关 → 解析目的地 → 冷却检查 → 发 Discord → 按结果处理队列。
async fn deliver_one(state: &AppState, job: BackupJob) {
    // 投递前再确认开关还在：成员中途关掉就丢弃，避免“关了还在发”。
    if !enabled(state, &job.member_id).await {
        state.backup.pop(&job.member_id);
        return;
    }
    let dest = match discord_share::destination(state, &job.member_id).await {
        Ok(Some(d)) => d,
        Ok(None) => {
            // 绑定没了：清空该成员积压，停止尝试；不回退公共频道。
            state.backup.mark_dead(&job.member_id);
            tracing::warn!(
                target: "backup",
                member = %job.member_id,
                request_id = %job.request_id,
                "auto-backup stopped: personal channel unbound"
            );
            return;
        }
        Err(_) => {
            // 凭据读取失败（解密/存储问题）：不动队列，等下一轮。
            state.backup.cooldown(&job.member_id, 15.0);
            return;
        }
    };
    if let Err(e) = discord_share::check_cooldown(state, &dest).await {
        // 已在冷却：按返回的 Retry-After 等，作业留在队首。
        state
            .backup
            .cooldown(&job.member_id, retry_seconds(&e).unwrap_or(15.0));
        return;
    }
    let (mime, ext) = job.sniff();
    let part = reqwest::multipart::Part::bytes(job.bytes.clone())
        .file_name(format!("owl-backup-{}.{}", job.image_index, ext))
        .mime_str(mime)
        .expect("fixed mime");
    // 备份消息不带生成参数：payload 只有 allowed_mentions，附件即最终图片。
    let form = reqwest::multipart::Form::new()
        .text(
            "payload_json",
            json!({"allowed_mentions":{"parse":[]}}).to_string(),
        )
        .part("files[0]", part);
    match discord_share::execute(state, &dest, form).await {
        Ok(message_id) => {
            state.backup.pop(&job.member_id);
            tracing::info!(
                target: "backup",
                member = %job.member_id,
                request_id = %job.request_id,
                image_index = job.image_index,
                bytes = job.bytes.len(),
                "auto-backup delivered, msg={}",
                message_id
            );
        }
        Err(e) => {
            if let Some(wait) = retry_seconds(&e) {
                // Discord 限流：按 Retry-After 等待，作业留在队首。
                state.backup.cooldown(&job.member_id, wait.min(3600.0));
                return;
            }
            if destination_dead(&e) {
                // 频道失效：清空积压并停用，提示重连（不回退公共频道）。
                state.backup.mark_dead(&job.member_id);
                tracing::warn!(
                    target: "backup",
                    member = %job.member_id,
                    request_id = %job.request_id,
                    "auto-backup destination dead; queue discarded, reconnect required"
                );
                return;
            }
            // 结果未知 / 其他错误：不自动重发（避免重复消息），丢弃并记日志，从浏览器历史手动补传。
            state.backup.pop(&job.member_id);
            tracing::error!(
                target: "backup",
                member = %job.member_id,
                request_id = %job.request_id,
                image_index = job.image_index,
                error = ?e,
                "auto-backup delivery uncertain; job dropped (restore manually from browser history)"
            );
        }
    }
}

/// 限流错误的等待秒数（只认 discord_share 的限流原因码）。
fn retry_seconds(e: &ApiError) -> Option<f64> {
    let g = match e {
        ApiError::Gate(g) if g.code == "discord_share_rate" => g,
        _ => return None,
    };
    Some(
        g.retry_after
            .unwrap_or(crate::error::retry_after_default(&g.code)) as f64,
    )
}

/// 目的地失效（discord_share::execute 把上游 401/403/404 映射成 409 提示重连）。
fn destination_dead(e: &ApiError) -> bool {
    matches!(e, ApiError::Http(h) if h.status == 409)
}

// ------------------------------------------------------------------ 成员端点

/// 成员端点：查看 / 设置自动备份开关（网页登录会话）。
pub fn router() -> axum::Router<AppState> {
    axum::Router::new().route(
        "/public/me/backup-settings",
        axum::routing::get(backup_status).put(backup_toggle),
    )
}

async fn web_member(state: &AppState, headers: &axum::http::HeaderMap) -> Result<String, ApiError> {
    let Some((id, key)) = crate::session::member_state(state, headers).await? else {
        return Err(crate::error::HttpError::new(401, "请先登录，再管理自动备份。").into());
    };
    if !key.is_some_and(|k| k.enabled()) {
        return Err(crate::error::HttpError::new(403, "你还没有可用的成员凭据。").into());
    }
    Ok(id)
}

async fn backup_status(
    axum::extract::State(state): axum::extract::State<AppState>,
    headers: axum::http::HeaderMap,
) -> Result<axum::response::Response, ApiError> {
    let id = web_member(&state, &headers).await?;
    Ok(crate::error::ok_json(&json!({
        "enabled": enabled(&state, &id).await,
        "connected": discord_share::destination(&state, &id).await?.is_some(),
    })))
}

#[derive(serde::Deserialize)]
struct BackupToggleBody {
    enabled: bool,
}

async fn backup_toggle(
    axum::extract::State(state): axum::extract::State<AppState>,
    headers: axum::http::HeaderMap,
    axum::extract::Json(body): axum::extract::Json<BackupToggleBody>,
) -> Result<axum::response::Response, ApiError> {
    let id = web_member(&state, &headers).await?;
    if !crate::session::origin_ok(&state, &headers) {
        return Err(crate::error::HttpError::new(403, "来源校验失败").into());
    }
    set_enabled(&state, &id, body.enabled).await?;
    Ok(crate::error::ok_json(
        &json!({"ok": true, "enabled": body.enabled}),
    ))
}

// ------------------------------------------------------------------ 非流式图片提取

/// 从已完整收到的生成响应里提取图片：单图直接返回，ZIP 逐项解出（顺序即图片顺序）。
/// 只做宽松提取——畸形项跳过；字节上限与开关检查交给 enqueue。
pub fn extract_images(body: &[u8]) -> Vec<Vec<u8>> {
    if !body.starts_with(b"PK") {
        return vec![body.to_vec()];
    }
    let Ok(mut archive) = zip::ZipArchive::new(std::io::Cursor::new(body)) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for i in 0..archive.len() {
        let Ok(mut entry) = archive.by_index(i) else {
            continue;
        };
        if entry.is_dir() {
            continue;
        }
        let mut buf = Vec::with_capacity(entry.size().min(MAX_IMAGE_BYTES as u64) as usize);
        if std::io::Read::read_to_end(&mut entry, &mut buf).is_ok()
            && !buf.is_empty()
            && buf.len() <= MAX_IMAGE_BYTES
        {
            out.push(buf);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(size: usize) -> Vec<u8> {
        let mut v = b"\x89PNG\r\n\x1a\n".to_vec();
        v.resize(size.max(8), 0xAB);
        v
    }

    #[test]
    fn extract_handles_zip_and_single_image() {
        // 单图（非 ZIP）原样返回。
        let img = png(64);
        assert_eq!(extract_images(&img), vec![img.clone()]);
        // ZIP：两项全解出，空项跳过。
        let mut buf = Vec::new();
        {
            let mut w = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            w.start_file("0.png", opts).unwrap();
            std::io::Write::write_all(&mut w, &png(32)).unwrap();
            w.start_file("1.png", opts).unwrap();
            std::io::Write::write_all(&mut w, &png(48)).unwrap();
            w.finish().unwrap();
        }
        let images = extract_images(&buf);
        assert_eq!(images.len(), 2);
        assert_eq!(images[0], png(32));
        assert_eq!(images[1], png(48));
        // 畸形 ZIP：返回空（不投递）。
        assert!(extract_images(b"PK\x03\x04garbage").is_empty());
    }

    #[test]
    fn queue_enforces_limits_and_dead_state() {
        let center = BackupCenter::default();
        // 每成员条数上限：第 9 张不入队。
        for i in 0..MAX_QUEUE_PER_MEMBER as i64 {
            center.enqueue("m1", "req", i, png(16));
        }
        center.enqueue("m1", "req", 99, png(16));
        assert_eq!(center.next().map(|j| j.image_index), Some(0));
        // 单张过大直接丢弃。
        center.enqueue("m2", "req", 0, png(MAX_IMAGE_BYTES + 1));
        assert!(center.next().is_none() || center.next().unwrap().member_id == "m1");
        // 死亡：清空积压，next 只认其他成员。
        center.mark_dead("m1");
        assert!(center.next().is_none());
        // pop 释放字节后可继续入队。
        let center2 = BackupCenter::default();
        center2.enqueue("m", "req", 0, png(16));
        assert!(center2.next().is_some());
        center2.pop("m");
        center2.enqueue("m", "req", 1, png(16));
        assert_eq!(center2.next().unwrap().image_index, 1);
    }
}
