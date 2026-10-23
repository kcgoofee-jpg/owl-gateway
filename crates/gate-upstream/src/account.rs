//! 账号查询（订阅 / Anlas 余额 / Token 校验）：只读，与出图同一套浏览器指纹，不碰调度、不占出图槽。

use crate::http::{self, Http};
use bytes::Bytes;
use serde_json::Value;
use std::time::Duration;
use wreq::header::HeaderMap;

/// 带 Token 的只读 GET（Accept: application/json），正文超过 `max_bytes` 报错。
pub(crate) async fn get_json(
    client: &Http,
    profile_index: usize,
    token: &str,
    url: &str,
    timeout: Duration,
    max_bytes: usize,
    follow_redirects: bool,
) -> Result<(u16, HeaderMap, Bytes), String> {
    let mut req = client
        .build(
            profile_index,
            token,
            wreq::Method::GET,
            url,
            None,
            "application/json",
        )
        .timeout(timeout);
    if !follow_redirects {
        req = req.redirect(wreq::redirect::Policy::none());
    }
    let resp = req.send().await.map_err(|e| e.to_string())?;
    let status = resp.status().as_u16();
    let headers = resp.headers().clone();
    match http::read_capped(resp, max_bytes).await {
        Ok(Some(body)) => Ok((status, headers, body)),
        Ok(None) => Err("响应过大".into()),
        Err(e) => Err(e.to_string()),
    }
}

/// 后台「添加 / 替换 Token」前的有效性检查：查订阅（只读，不消耗额度，不记录 Token）。返回订阅等级。
pub(crate) async fn verify(
    client: &Http,
    profile_index: usize,
    image_host: &str,
    token: &str,
) -> Result<i64, String> {
    let url = format!("{image_host}/user/subscription");
    let (status, _, body) = get_json(
        client,
        profile_index,
        token,
        &url,
        Duration::from_secs(12),
        1 << 20,
        true,
    )
    .await
    .map_err(|_| "无法连接 NovelAI 验证这把 Token，请稍后再试".to_string())?;
    if status == 401 || status == 403 {
        return Err("NovelAI 拒绝了这把 Token（无效，或已被重置）".into());
    }
    if status != 200 {
        return Err(format!("NovelAI 返回 {status}，暂时无法验证"));
    }
    let unknown = || "NovelAI 返回了无法识别的订阅信息".to_string();
    let info: Value = serde_json::from_slice(&body).map_err(|_| unknown())?;
    if !info.is_object() {
        return Err(unknown());
    }
    let tier = info
        .get("tier")
        .map_or(Some(0), Value::as_i64)
        .ok_or_else(unknown)?;
    if info.get("active").and_then(Value::as_bool) != Some(true) {
        return Err("这把 Token 对应的账号没有有效订阅".into());
    }
    Ok(tier)
}
