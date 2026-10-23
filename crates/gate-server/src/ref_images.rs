//! 参考图哈希（站长 10-23：防一人多号 / 共用 Key）：成员上传的图生图底图、Vibe 参考图、精确参考图、
//! Vibe 编码的原图，各算一个 sha256（原始字节）和一个 64 位 dHash（解码后的感知哈希，轻微压缩 / 缩放后仍接近），
//! 记进 `ref_images`。不存图片本身。不同 Key 用了同一张或几乎一样的参考图，由关联扫描写成待审信号。
//!
//! `*_cached` 一类字段是上游缓存键、不是图片，不算。

use crate::reqctx::ReqCtx;
use crate::state::AppState;
use base64::Engine;
use gate_db::repo::footprint::RefImage;
use serde_json::Value;
use sha2::{Digest, Sha256};

/// 一次请求最多算几张（Vibe 最多 16 张，加底图 / 精确参考留点余量）。
const MAX_IMAGES: usize = 24;

/// 出图请求体 `parameters` 里的参考图：`(种类, base64 文本)`。
pub fn images_in_params(params: Option<&Value>) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    let Some(p) = params else {
        return out;
    };
    if let Some(s) = p.get("image").and_then(Value::as_str) {
        out.push(("img2img", s.to_string()));
    }
    for (field, kind) in [
        ("reference_image_multiple", "vibe"),
        ("director_reference_images", "precise_ref"),
    ] {
        if let Some(a) = p.get(field).and_then(Value::as_array) {
            out.extend(
                a.iter()
                    .filter_map(Value::as_str)
                    .map(|s| (kind, s.to_string())),
            );
        }
    }
    out.retain(|(_, s)| !s.is_empty());
    out.truncate(MAX_IMAGES);
    out
}

/// 64 位 dHash：灰度、缩到 9x8，每行相邻像素左亮于右记 1。返回 16 位十六进制；解不出图为 None。
pub fn dhash(bytes: &[u8]) -> Option<String> {
    let img = image::load_from_memory(bytes).ok()?;
    let small = img
        .grayscale()
        .resize_exact(9, 8, image::imageops::FilterType::Triangle)
        .to_luma8();
    let mut bits: u64 = 0;
    for y in 0..8 {
        for x in 0..8 {
            let l = small.get_pixel(x, y)[0];
            let r = small.get_pixel(x + 1, y)[0];
            bits = (bits << 1) | u64::from(l > r);
        }
    }
    Some(format!("{bits:016x}"))
}

/// 两个 dHash 的汉明距离（格式不对返回 None）。
pub fn hamming(a: &str, b: &str) -> Option<u32> {
    let a = u64::from_str_radix(a, 16).ok()?;
    let b = u64::from_str_radix(b, 16).ok()?;
    Some((a ^ b).count_ones())
}

/// 一张图的哈希：base64（可带 `data:...;base64,` 前缀）解得出来就对字节算，解不出来就对原文算 sha256（dHash 为空）。
pub fn hash_one(kind: &'static str, b64: &str) -> RefImage {
    let text = b64.split_once(";base64,").map_or(b64, |(_, t)| t).trim();
    let bytes = base64::engine::general_purpose::STANDARD.decode(text).ok();
    let raw = bytes.as_deref().unwrap_or(text.as_bytes());
    RefImage {
        kind,
        sha256: hex::encode(Sha256::digest(raw)),
        dhash: bytes.as_deref().and_then(dhash),
    }
}

impl AppState {
    /// 后台算这次请求里参考图的哈希并入库（解码在阻塞线程里做；任何失败都不影响请求）。
    pub fn record_ref_images(
        &self,
        ctx: &ReqCtx,
        key_id: i64,
        images: Vec<(&'static str, String)>,
    ) {
        if images.is_empty() {
            return;
        }
        let (state, rid) = (self.clone(), ctx.rid());
        self.tasks.spawn(async move {
            let Ok(rows) = tokio::task::spawn_blocking(move || {
                images
                    .iter()
                    .map(|(k, s)| hash_one(k, s))
                    .collect::<Vec<_>>()
            })
            .await
            else {
                return;
            };
            if let Err(e) = state
                .db
                .record_ref_images(&rid, key_id, &rows, state.now())
                .await
            {
                tracing::warn!("记参考图哈希失败：{e}");
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn png(w: u32, h: u32, f: impl Fn(u32, u32) -> u8) -> Vec<u8> {
        let img = image::GrayImage::from_fn(w, h, |x, y| image::Luma([f(x, y)]));
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    #[test]
    fn dhash_is_stable_under_resize_and_differs_for_other_images() {
        let grad = |w: u32| move |x: u32, y: u32| ((x * 255 / w) as u8) ^ ((y % 7) as u8 * 9);
        let a = dhash(&png(256, 256, grad(256))).unwrap();
        let b = dhash(&png(128, 128, grad(128))).unwrap();
        assert_eq!(a.len(), 16);
        assert!(hamming(&a, &b).unwrap() <= 6, "{a} vs {b}");
        let other = dhash(&png(256, 256, |x, y| {
            if (x / 32 + y / 32) % 2 == 0 { 0 } else { 255 }
        }))
        .unwrap();
        assert!(hamming(&a, &other).unwrap() > 6);
        assert_eq!(dhash(b"not an image"), None);
    }

    #[test]
    fn collects_images_and_skips_cached_keys() {
        let body = json!({"parameters": {
            "image": "aW1n",
            "reference_image_multiple": ["djE=", "", "djI="],
            "reference_image_multiple_cached": [{"data": "x"}],
            "director_reference_images": ["cDE="],
            "director_reference_images_cached": [{"cache_secret_key": "k"}],
            "mask": "bWFzaw==",
        }});
        let got = images_in_params(body.get("parameters"));
        let got: Vec<(&str, &str)> = got.iter().map(|(k, s)| (*k, s.as_str())).collect();
        assert_eq!(
            got,
            [
                ("img2img", "aW1n"),
                ("vibe", "djE="),
                ("vibe", "djI="),
                ("precise_ref", "cDE=")
            ]
        );
        assert!(images_in_params(None).is_empty());
    }

    #[test]
    fn hash_one_hashes_decoded_bytes_and_tolerates_bad_base64() {
        let r = hash_one("vibe", "data:image/png;base64,aGVsbG8=");
        assert_eq!(r.sha256, hex::encode(Sha256::digest(b"hello")));
        assert_eq!(r.dhash, None);
        let bad = hash_one("vibe", "%%%");
        assert_eq!(bad.sha256, hex::encode(Sha256::digest(b"%%%")));
        let p = png(64, 64, |x, _| (x * 4) as u8);
        let b64 = base64::engine::general_purpose::STANDARD.encode(&p);
        assert!(hash_one("img2img", &b64).dhash.is_some());
    }
}
