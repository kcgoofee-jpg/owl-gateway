//! `GET /v1/me`：这把 Key 自己的额度、功能开关和闲置回收设置（旧版 `app/routers/text_routes.py` 里的 `/v1/me`）。
//!
//! 2026-10-15 文本生成 / 语音 / OpenAI 兼容聊天整套接口删除（线上几乎没人用），这个文件原先叫
//! `text.rs`，只留下了 `/v1/me`（线上冒烟 `nai-ops smoke` 和用 Key 自查额度的脚本在用）。

use crate::auth::{authenticate, require_feature};
use crate::error::{ApiError, ApiResult, ok_json};
use crate::key::Key;
use crate::state::AppState;
use axum::Router;
use axum::extract::{Request, State};
use axum::routing::get;
use gate_core::features::FEATURES;
use gate_core::pyc;

use serde_json::{Value, json};

pub fn router() -> Router<AppState> {
    Router::new().route("/v1/me", get(v1_me))
}

/// `_feature_view`：该 Key 当前各项功能是否可用（已计入全局开关）。
async fn feature_view(state: &AppState, key: &Key) -> Result<Vec<Value>, ApiError> {
    let mut view = Vec::new();
    for (name, label) in FEATURES {
        // require_feature 只在被拒时返回 Gate 错误（对应旧版 `check(...) is not None`）
        let on = match require_feature(state, key, name).await {
            Ok(()) => true,
            Err(ApiError::Gate(_)) => false,
            Err(e) => return Err(e),
        };
        view.push(json!({"id": name, "label": label, "on": on}));
    }
    Ok(view)
}

async fn v1_me(State(state): State<AppState>, req: Request) -> ApiResult {
    let (parts, _) = req.into_parts();
    let key = authenticate(&state, &parts, false).await?;
    let c = state.db.get_counter(key.id(), &state.day()).await?;
    let generated = state.db.generated_image_totals(Some(key.id())).await?;
    let generated_total = generated
        .iter()
        .find(|(id, _)| *id == key.id())
        .map_or(0, |(_, n)| *n);
    let anlas_today = c.anlas;
    let anlas_month = state.db.month_anlas(key.id(), &state.month()).await?;
    let features = feature_view(&state, &key).await?;
    // 3.0 去掉的字段：`daily_images_base`（保底）、`anlas_auto`（V5 续杯）
    Ok(ok_json(&json!({
        "name": key.name(),
        "generated_images_total": generated_total,
        "today": {
            "images": c.images,
            "daily_images": key.get("daily_images"),
            "legacy_free_images_today": c.legacy_free_images,
            "anlas_today": pyc::round(anlas_today, 2),
            "daily_anlas": key.get("daily_anlas"),
            "v5_today": c.v5,
            "daily_v5": key.get("daily_v5"),
            "image_model_scope": key.get("image_model_scope"),
            "anlas_month": pyc::round(anlas_month, 2),
            "monthly_anlas": key.get("monthly_anlas"),
            "text_tokens": c.text_tokens,
            "daily_text_tokens": key.get("daily_text_tokens"),
            "requests": c.requests,
        },
        "expires_at": key.get("expires_at"),
        "inactivity_delete_days": state.env.key_inactivity_delete_days,
        "features": features,
    })))
}
