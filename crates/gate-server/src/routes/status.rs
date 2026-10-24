//! 公开状态页：`GET|HEAD /status`（HTML）、`GET /status.json`、
//! 事件没有后台接口，直接写库（`incidents` / `incident_updates` 表）。
//!
//! - JSON 字段与取值是固定契约（第三方可能存档 `/status.json`）；HTML 的安全头与样式加载方式见 [`page`]。
//! - 公开页不登录就能访问：60 秒内复用同一份快照，避免被反复刷新时每次都扫 90 天日志（缓存挂在路由上，
//!   不是全局变量；`app()` 只组装一次路由，所以线上缓存是共享的）。
//! - 样式与主题切换脚本编进二进制，由 `/_owl/status-page.{css,js}` 提供（地址带内容哈希，长期缓存）：
//!   CSP 是 `style-src 'self'; script-src 'self'`，页面里没有内联样式 / 脚本。
//!   样式 = 首页的设计令牌（`web/src/ui/tokens.css`）+ 基础样式（`web/src/ui/base.css`）+ 顶栏 / 转场（`web/src/ui/chrome.css`）+ 状态页自己的版面，
//!   编译时拼成一份：首页换了令牌，状态页跟着换。

mod feed;
mod page;
pub mod snapshot;

pub use feed::{STATUS_LABEL, incident_json, standard_body};
pub use snapshot::{COMPONENTS, OFFICIAL_KEY, Snapshot};

use crate::error::ApiError;
use crate::state::AppState;
use axum::Router;
use axum::body::Body;
use axum::extract::{Extension, RawQuery, State};
use axum::http::{HeaderValue, header};
use axum::response::Response;
use axum::routing::get;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::Arc;

/// 快照复用时长（秒）。
const SNAPSHOT_TTL: f64 = 60.0;

const CSS: &str = concat!(
    include_str!("../../../../web/src/ui/tokens.css"),
    include_str!("../../../../web/src/ui/base.css"),
    include_str!("../../../../web/src/ui/chrome.css"),
    include_str!("status/status-page.css"),
);
const JS: &str = include_str!("status/status-page.js");

/// 页面的内容安全策略：只允许同源样式 / 脚本，不允许被嵌入，不允许任何表单提交。
pub const STATUS_CSP: &str = "default-src 'none'; style-src 'self'; script-src 'self'; img-src 'self'; \
    base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

/// NovelAI 官方状态页（status.io）的公开接口，不涉及账号。
pub const OFFICIAL_API: &str = "https://api.status.io/1.0/status/654839612cedb404d4d5f578";

/// 挂在路由上的共享数据：快照缓存 + 静态资源的内容哈希。
struct Shared {
    slot: tokio::sync::Mutex<Option<(f64, Arc<Snapshot>)>>,
    css_href: String,
    js_href: String,
    css_v: String,
    js_v: String,
}

impl Shared {
    fn new() -> Self {
        let v = |s: &str| hex::encode(Sha256::digest(s.as_bytes()))[..12].to_string();
        let (css_v, js_v) = (v(CSS), v(JS));
        Self {
            slot: tokio::sync::Mutex::new(None),
            css_href: format!("/_owl/status-page.css?v={css_v}"),
            js_href: format!("/_owl/status-page.js?v={js_v}"),
            css_v,
            js_v,
        }
    }

    async fn snapshot(&self, st: &AppState) -> Result<Arc<Snapshot>, ApiError> {
        let now = st.now();
        let mut slot = self.slot.lock().await;
        if let Some((at, snap)) = slot.as_ref()
            && now - at <= SNAPSHOT_TTL
        {
            return Ok(snap.clone());
        }
        let snap = Arc::new(load(st, now).await?);
        *slot = Some((now, snap.clone()));
        Ok(snap)
    }
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/status", get(status_page))
        .route("/status.json", get(status_json))
        .route("/_owl/status-page.css", get(css))
        .route("/_owl/status-page.js", get(js))
        .layer(Extension(Arc::new(Shared::new())))
}

/// 从库里读出数据并组装快照。
async fn load(st: &AppState, now: f64) -> Result<Snapshot, gate_db::DbError> {
    use snapshot::*;
    let clock = &st.clock;
    let first = days_before(local_date(clock, now), DAYS - 1);
    let start = midnight(clock, first);
    let incidents = st.db.list_incidents(start).await?;
    let hourly = st.db.status_hourly_since(start as i64).await?;
    let buckets = st.db.upstream_buckets_since(start).await?;
    let upstream_recent = st.db.upstream_recent(now - 3600.0).await?;
    let official = st.db.get_setting(OFFICIAL_KEY).await?;
    Ok(build(
        clock,
        now,
        Inputs {
            incidents,
            api_error_days: api_error_days(clock, &hourly),
            upstream_days: upstream_days(clock, &buckets),
            upstream_recent,
            official_image_code: official_image_code(official.as_deref(), now),
        },
    ))
}

/// 带显式 Content-Length 的响应：HEAD 请求（axum 会去掉正文）也能拿到与 GET 相同的长度。
fn respond(
    content_type: &'static str,
    body: Vec<u8>,
    headers: &[(&'static str, &'static str)],
) -> Response {
    let len = body.len();
    let mut resp = Response::new(Body::from(body));
    let h = resp.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    h.insert(header::CONTENT_LENGTH, HeaderValue::from(len));
    for (k, v) in headers {
        h.insert(*k, HeaderValue::from_static(v));
    }
    resp
}

/// 「加入 Discord 获取更新」的地址：
/// 1. 站点邀请链接（`DISCORD_INVITE_URL`）——匿名可加入，是真邀请；
/// 2. 没配邀请就退回公告频道（老行为）；
/// 3. 都没有时 `None`（不放一个指回 `/status` 自己的假链接）。
fn subscribe_url(st: &AppState) -> Option<String> {
    let invite = st.env.discord_invite_url.trim();
    if !invite.is_empty() {
        return Some(invite.to_string());
    }
    let (guild, chan) = (&st.env.discord_guild_id, &st.env.announce_channel_id);
    if !guild.is_empty() && !chan.is_empty() {
        Some(format!("https://discord.com/channels/{guild}/{chan}"))
    } else {
        None
    }
}

async fn status_page(
    State(st): State<AppState>,
    Extension(sh): Extension<Arc<Shared>>,
) -> Result<Response, ApiError> {
    let snap = sh.snapshot(&st).await?;
    let assets = page::AssetRefs {
        css: &sh.css_href,
        js: &sh.js_href,
    };
    let subscribe = subscribe_url(&st);
    let html = page::render(&snap, &st.clock, subscribe.as_deref(), &assets).into_string();
    Ok(respond(
        "text/html; charset=utf-8",
        html.into_bytes(),
        &[
            ("cache-control", "no-cache"),
            ("x-content-type-options", "nosniff"),
            ("content-security-policy", STATUS_CSP),
            ("x-frame-options", "DENY"),
            ("referrer-policy", "no-referrer"),
        ],
    ))
}

async fn status_json(
    State(st): State<AppState>,
    Extension(sh): Extension<Arc<Shared>>,
) -> Result<Response, ApiError> {
    let snap = sh.snapshot(&st).await?;
    Ok(respond(
        "application/json",
        serde_json::to_vec(&feed::status_json(&snap)).unwrap_or_default(),
        &[("cache-control", "no-cache")],
    ))
}

/// 静态资源：地址里的 `?v=` 与当前内容哈希一致才长期缓存（页面每次都引用最新的哈希）。
fn asset(
    content_type: &'static str,
    body: &'static str,
    v: &str,
    query: Option<String>,
) -> Response {
    let fresh = query
        .as_deref()
        .is_some_and(|q| q.split('&').any(|p| p == format!("v={v}")));
    let cache = if fresh {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    respond(
        content_type,
        body.as_bytes().to_vec(),
        &[
            ("cache-control", cache),
            ("x-content-type-options", "nosniff"),
        ],
    )
}

async fn css(Extension(sh): Extension<Arc<Shared>>, RawQuery(q): RawQuery) -> Response {
    asset("text/css; charset=utf-8", CSS, &sh.css_v, q)
}

async fn js(Extension(sh): Extension<Arc<Shared>>, RawQuery(q): RawQuery) -> Response {
    asset("text/javascript; charset=utf-8", JS, &sh.js_v, q)
}

/// 官方状态接口的返回 → 存进设置的那份摘要（字段与顺序固定，快照测试按它比较）。
/// 形状不对（缺 `result` / `status_overall`）返回 None，调用方保留上次的结果。
pub fn official_value(resp: &Value, fetched_at: f64) -> Option<Value> {
    let d = resp.get("result")?;
    let overall = d.get("status_overall")?;
    let img = d
        .get("status")
        .and_then(Value::as_array)
        .and_then(|a| {
            a.iter()
                .find(|x| x.get("name").and_then(Value::as_str) == Some("Image Generation"))
        })
        .cloned()
        .unwrap_or_else(|| json!({}));
    let text = |v: &Value, k: &str| v.get(k).cloned().unwrap_or_else(|| json!(""));
    Some(json!({
        "overall": text(overall, "status"),
        "image": text(&img, "status"),
        "image_code": img.get("status_code").cloned().unwrap_or_else(|| json!(0)),
        "updated": text(overall, "updated"),
        "fetched_at": gate_core::pyval::float(fetched_at),
    }))
}

/// 取一次 NovelAI 官方状态（status.io 公开接口，不是出图接口）。失败就保留上次的结果。
/// 维护循环每 5 分钟调用一次。
pub async fn fetch_official(db: &gate_db::Db, url: &str, now: f64) {
    let Ok(client) = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(8))
        .redirect(reqwest::redirect::Policy::none())
        .build()
    else {
        return;
    };
    let Ok(resp) = client.get(url).send().await else {
        return;
    };
    let Ok(body) = resp.json::<Value>().await else {
        return;
    };
    if let Some(val) = official_value(&body, now)
        && let Ok(raw) = serde_json::to_string(&val)
    {
        let _ = db.set_setting(OFFICIAL_KEY, &raw).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn css_has_no_inline_style_dependency() {
        // 页面不再有 style 属性；样式表自己也不能引用外部资源
        assert!(!CSS.contains("@import") && !CSS.contains("url("));
        assert!(!STATUS_CSP.contains("unsafe-inline") && !STATUS_CSP.contains("unsafe-eval"));
        for need in [
            "frame-ancestors 'none'",
            "base-uri 'none'",
            "script-src 'self'",
            "style-src 'self'",
        ] {
            assert!(STATUS_CSP.contains(need), "{need}");
        }
    }

    #[test]
    fn official_summary_shape() {
        let resp = json!({"result": {
            "status_overall": {"status": "Operational", "updated": "2026-10-14T00:00:00Z"},
            "status": [
                {"name": "Website", "status": "Operational", "status_code": 100},
                {"name": "Image Generation", "status": "Degraded", "status_code": 300},
            ],
        }});
        let v = official_value(&resp, 1_791_950_400.5).unwrap();
        assert_eq!(
            v,
            json!({"overall": "Operational", "image": "Degraded", "image_code": 300, "updated": "2026-10-14T00:00:00Z", "fetched_at": 1791950400.5})
        );
        // 没有「Image Generation」：image 为空串、状态码 0
        let resp = json!({"result": {"status_overall": {"status": "Operational"}, "status": []}});
        let v = official_value(&resp, 1.0).unwrap();
        assert_eq!(v["image"], "");
        assert_eq!(v["image_code"], 0);
        assert_eq!(v["updated"], "");
        assert!(official_value(&json!({"nope": 1}), 1.0).is_none());
    }

    #[test]
    fn asset_cache_follows_hash() {
        let sh = Shared::new();
        let hashed = asset("text/css", CSS, &sh.css_v, Some(format!("v={}", sh.css_v)));
        assert_eq!(
            hashed.headers()["cache-control"],
            "public, max-age=31536000, immutable"
        );
        let stale = asset("text/css", CSS, &sh.css_v, Some("v=old".into()));
        assert_eq!(stale.headers()["cache-control"], "no-cache");
        let bare = asset("text/css", CSS, &sh.css_v, None);
        assert_eq!(bare.headers()["cache-control"], "no-cache");
        assert_eq!(sh.css_href, format!("/_owl/status-page.css?v={}", sh.css_v));
    }

    /// 从设计令牌的一段（`:root {` 或 `:root[data-theme="dark"] {`）里取出颜色变量。
    fn tokens(css: &str, marker: &str) -> std::collections::HashMap<String, String> {
        let start = css.find(marker).unwrap_or_else(|| panic!("缺少 {marker}")) + marker.len();
        let body = &css[start..];
        let end = body.find('}').expect("令牌块没有结束");
        body[..end]
            .lines()
            .filter_map(|line| {
                let line = line.trim();
                let (name, value) = line.strip_prefix("--")?.split_once(':')?;
                let value = value.trim().trim_end_matches(';').trim();
                value
                    .starts_with('#')
                    .then(|| (format!("--{}", name.trim()), value.to_string()))
            })
            .collect()
    }

    fn luminance(hex: &str) -> f64 {
        let h = hex.trim_start_matches('#');
        let channel = |i: usize| {
            let c = u8::from_str_radix(&h[i..i + 2], 16).expect("颜色不是 #rrggbb") as f64 / 255.0;
            if c <= 0.03928 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(0) + 0.7152 * channel(2) + 0.0722 * channel(4)
    }

    fn contrast(a: &str, b: &str) -> f64 {
        let (l1, l2) = {
            let (x, y) = (luminance(a), luminance(b));
            if x > y { (x, y) } else { (y, x) }
        };
        (l1 + 0.05) / (l2 + 0.05)
    }

    /// 状态页实际用到的「文字色 / 背景色」对，亮色与暗色都要 >= 4.5:1（WCAG AA 正文）。
    /// axe 的 254 条 color-contrast 主要是 `.reveal` 的 `opacity: .4` + `blur(10px)` 造成的；
    /// 这里把去掉 reveal 之后的令牌本身也锁住，避免以后换色又掉到 AA 以下。
    #[test]
    fn text_colors_meet_wcag_aa() {
        let pairs = [
            ("--text", "--bg"),             // 正文（事件标题、进展正文）
            ("--text", "--surface"),        // 卡片里的正文
            ("--text-2", "--surface"),      // 日期小标题 / 事件摘要
            ("--text-2", "--surface-3"),    // 状态徽章（.pill）
            ("--muted", "--bg"),            // 时间（small）/ 页脚
            ("--muted", "--surface"),       // 卡片里的时间
            ("--ok", "--ok-soft"),          // 「已解决」徽章
            ("--ok", "--surface"),          // 组件状态「运行正常」
            ("--warn", "--surface"),        // 性能下降 / minor 标题
            ("--danger", "--surface"),      // 严重中断 / major 标题
            ("--orange", "--surface"),      // 部分中断（色条第 2 档）
            ("--accent-text", "--surface"), // 正文里的链接（页脚导航）
        ];
        let light = tokens(CSS, ":root {");
        let dark = tokens(CSS, ":root[data-theme=\"dark\"] {");
        for (scheme, map) in [("light", &light), ("dark", &dark)] {
            for (fg, bg) in pairs {
                let (f, b) = (
                    map.get(fg).unwrap_or_else(|| panic!("{scheme} 缺少 {fg}")),
                    map.get(bg).unwrap_or_else(|| panic!("{scheme} 缺少 {bg}")),
                );
                let ratio = contrast(f, b);
                assert!(
                    ratio >= 4.5,
                    "{scheme}: {fg} on {bg} = {ratio:.2}:1，低于 WCAG AA 的 4.5:1"
                );
            }
        }
        // 事件链接要有下划线（不只靠颜色区分；正文里没有别的下划线链接）
        assert!(CSS.contains(".sp-inc-title a"));
        assert!(CSS.contains("text-decoration: underline"));
        // 状态页自己不能再用 `.reveal`（web/src/ui/chrome.css 的 blur + opacity 低对比）
        assert!(!CSS.contains(".sp-day.reveal"));
    }
}
