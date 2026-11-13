//! Discord CDN images are bounded and inlined in memory; no image is saved or logged.
use crate::chat::ChatError;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use reqwest::{Client, Url};
use serde_json::Value;
use std::{sync::OnceLock, time::Duration};

const INPUT_IMAGE_LIMIT: usize = 10 * 1024 * 1024;
const INPUT_TOTAL_LIMIT: usize = 20 * 1024 * 1024;
pub(crate) const IMAGE_LIMIT: usize = 4 * 1024 * 1024;
const TOTAL_LIMIT: usize = 8 * 1024 * 1024;

pub(crate) fn has_images(user: &Value) -> bool {
    user.as_array()
        .is_some_and(|parts| parts.iter().any(|p| p["type"] == "image_url"))
}

fn image_mime(bytes: &[u8]) -> Result<&'static str, ChatError> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Ok("image/png")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Ok("image/jpeg")
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        Ok("image/webp")
    } else {
        Err(ChatError::ImageInput(
            "图片格式不支持，请上传 PNG、JPG 或 WebP",
        ))
    }
}

fn normalize(bytes: Vec<u8>) -> Result<(&'static str, Vec<u8>), ChatError> {
    use image::{ImageFormat, ImageReader};
    use std::io::Cursor;
    image_mime(&bytes)?;
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| ChatError::ImageInput("图片格式无效"))?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(96 * 1024 * 1024);
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    reader.limits(limits);
    let decoded = reader
        .decode()
        .map_err(|_| ChatError::ImageInput("图片损坏或分辨率过大，请压缩后上传"))?;
    let decoded = if decoded.width() > 2048 || decoded.height() > 2048 {
        decoded.thumbnail(2048, 2048)
    } else {
        decoded
    };
    // Opaque RGBA uploads should use JPEG too; retaining a redundant alpha channel
    // can make an ordinary generated PNG exceed the normalized payload budget.
    let alpha = decoded.color().has_alpha() && decoded.to_rgba8().pixels().any(|p| p[3] != 255);
    let decoded = if alpha && (decoded.width() > 1000 || decoded.height() > 1000) {
        decoded.thumbnail(1000, 1000)
    } else {
        decoded
    };
    let mut output = Cursor::new(Vec::new());
    if alpha {
        image::DynamicImage::ImageRgba8(decoded.to_rgba8())
            .write_to(&mut output, ImageFormat::Png)
            .map_err(|_| ChatError::ImageInput("图片转换失败"))?;
    } else {
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut output, 85)
            .encode_image(&decoded.to_rgb8())
            .map_err(|_| ChatError::ImageInput("图片转换失败"))?;
    }
    let bytes = output.into_inner();
    if bytes.len() > IMAGE_LIMIT {
        return Err(ChatError::ImageInput(
            "图片转换后超过大小限制，请压缩后上传",
        ));
    }
    Ok((if alpha { "image/png" } else { "image/jpeg" }, bytes))
}

fn discord_image_url(raw: &str) -> Result<Url, ChatError> {
    let url = Url::parse(raw).map_err(|_| ChatError::ImageInput("图片链接无效"))?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some_and(|port| port != 443)
        || !matches!(
            url.host_str(),
            Some("cdn.discordapp.com" | "media.discordapp.net")
        )
    {
        return Err(ChatError::ImageInput("图片来源无效，请直接上传到 Discord"));
    }
    Ok(url)
}

async fn download(client: &Client, url: Url, limit: usize) -> Result<Vec<u8>, ChatError> {
    let mut response = client
        .get(url)
        .send()
        .await
        .map_err(|_| ChatError::ImageInput("图片下载失败，请重新上传"))?;
    if !response.status().is_success() {
        return Err(ChatError::ImageInput("图片已失效或无法读取，请重新上传"));
    }
    if response.content_length().is_some_and(|n| n > limit as u64) {
        return Err(ChatError::ImageInput("图片超过大小限制"));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| ChatError::ImageInput("图片读取中断，请重新上传"))?
    {
        if chunk.len() > limit - bytes.len() {
            return Err(ChatError::ImageInput("图片超过大小限制"));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

pub(crate) async fn inline(mut user: Value) -> Result<Value, ChatError> {
    let Some(parts) = user.as_array_mut() else {
        return Ok(user);
    };
    static CLIENT: OnceLock<Client> = OnceLock::new();
    let client = CLIENT.get_or_init(|| {
        Client::builder()
            .timeout(Duration::from_secs(10))
            .connect_timeout(Duration::from_secs(3))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("图片客户端配置固定")
    });
    let mut remaining = INPUT_TOTAL_LIMIT;
    let mut encoded_remaining = TOTAL_LIMIT;
    let mut count = 0;
    for part in parts {
        if part["type"] != "image_url" {
            continue;
        }
        count += 1;
        if count > 4 {
            return Err(ChatError::ImageInput("一次最多读取四张图片"));
        }
        let raw = part["image_url"]["url"]
            .as_str()
            .ok_or(ChatError::ImageInput("图片链接无效"))?;
        let limit = INPUT_IMAGE_LIMIT.min(remaining);
        let bytes = if raw.starts_with("data:") {
            if raw.len() > limit.saturating_mul(4) / 3 + 128 {
                return Err(ChatError::ImageInput("图片超过大小限制"));
            }
            let (_, data) = raw
                .split_once(";base64,")
                .ok_or(ChatError::ImageInput("图片编码无效"))?;
            let decoded = STANDARD
                .decode(data)
                .map_err(|_| ChatError::ImageInput("图片编码无效"))?;
            if decoded.len() > limit {
                return Err(ChatError::ImageInput("图片超过大小限制"));
            }
            decoded
        } else {
            download(client, discord_image_url(raw)?, limit).await?
        };
        remaining -= bytes.len();
        static DECODE: OnceLock<std::sync::Arc<tokio::sync::Semaphore>> = OnceLock::new();
        let permit = DECODE
            .get_or_init(|| std::sync::Arc::new(tokio::sync::Semaphore::new(1)))
            .clone()
            .acquire_owned()
            .await
            .expect("图片解码池不会关闭");
        let (mime, bytes) = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            normalize(bytes)
        })
        .await
        .map_err(|_| ChatError::ImageInput("图片转换失败"))??;
        if bytes.len() > encoded_remaining {
            return Err(ChatError::ImageInput("图片合计超过大小限制"));
        }
        encoded_remaining -= bytes.len();
        part["image_url"]["url"] =
            Value::String(format!("data:{mime};base64,{}", STANDARD.encode(&bytes)));
    }
    Ok(user)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn only_exact_discord_https_origins_are_downloaded() {
        assert!(discord_image_url("https://cdn.discordapp.com/attachments/a.png?ex=1").is_ok());
        assert!(discord_image_url("https://media.discordapp.net/attachments/a.png").is_ok());
        for url in [
            "http://cdn.discordapp.com/a.png",
            "https://cdn.discordapp.com.evil.test/a.png",
            "https://cdn.discordapp.com@127.0.0.1/a.png",
            "http://127.0.0.1/a.png",
            "file:///etc/passwd",
            "https://cdn.discordapp.com:8443/a.png",
        ] {
            assert!(discord_image_url(url).is_err(), "{url}");
        }
    }
    #[tokio::test]
    async fn inline_data_uses_the_actual_image_format_and_preserves_text() {
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            2,
            2,
            image::Rgba([255, 0, 0, 128]),
        ))
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
        let data = STANDARD.encode(png.into_inner());
        let v=inline(json!([{"type":"text","text":"synthetic"},{"type":"image_url","image_url":{"url":format!("data:image/jpeg;base64,{data}")}}])).await.unwrap();
        assert_eq!(v[0]["text"], "synthetic");
        assert!(
            v[1]["image_url"]["url"]
                .as_str()
                .unwrap()
                .starts_with("data:image/png;base64,")
        );
        assert_eq!(
            inline(json!("text only")).await.unwrap(),
            json!("text only")
        );
        assert!(inline(json!([{"type":"image_url","image_url":{"url":"data:image/png;base64,bm90IGFuIGltYWdl"}}])).await.is_err());
    }
    #[tokio::test]
    async fn download_enforces_actual_stream_size_even_without_content_length_and_does_not_follow_redirects()
     {
        use axum::{Router, body::Body, response::Response};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let router = Router::new()
            .route(
                "/stream",
                axum::routing::get(|| async {
                    let chunks = futures::stream::iter([
                        Ok::<_, std::io::Error>(vec![0u8; 9]),
                        Ok(vec![0u8; 9]),
                    ]);
                    Response::builder().body(Body::from_stream(chunks)).unwrap()
                }),
            )
            .route(
                "/redirect",
                axum::routing::get(|| async {
                    Response::builder()
                        .status(302)
                        .header("location", "/stream")
                        .body(Body::empty())
                        .unwrap()
                }),
            );
        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        assert!(
            download(
                &client,
                format!("http://{address}/stream").parse().unwrap(),
                16
            )
            .await
            .is_err()
        );
        assert!(
            download(
                &client,
                format!("http://{address}/redirect").parse().unwrap(),
                100
            )
            .await
            .is_err()
        );
    }
}

#[cfg(test)]
mod large_image_tests {
    use super::*;
    #[test]
    fn phone_size_jpeg_is_bounded_and_normalized_for_vision() {
        let source = image::RgbImage::from_pixel(3376, 2368, image::Rgb([255, 0, 0]));
        let mut input = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut input, 90)
            .encode_image(&source)
            .unwrap();
        let (mime, output) = normalize(input).unwrap();
        let decoded = image::load_from_memory(&output).unwrap();
        assert_eq!(mime, "image/jpeg");
        assert!(decoded.width() <= 2048 && decoded.height() <= 2048);
        assert!(output.len() <= IMAGE_LIMIT);
    }
}

#[cfg(test)]
mod ordinary_upload_tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn ordinary_four_k_rgba_images_fit_the_bounded_decoder() {
        let picture = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            4096,
            4096,
            image::Rgba([255, 0, 0, 255]),
        ));
        let mut encoded = std::io::Cursor::new(Vec::new());
        picture
            .write_to(&mut encoded, image::ImageFormat::Png)
            .unwrap();
        drop(picture);
        let (mime, result) = normalize(encoded.into_inner()).unwrap();
        assert_eq!(mime, "image/jpeg");
        assert_eq!(image::load_from_memory(&result).unwrap().width(), 2048);
    }

    #[tokio::test]
    async fn opaque_png_above_four_megabytes_is_accepted_and_compacted() {
        let mut seed = 0x1234_5678u32;
        let picture = image::RgbaImage::from_fn(1536, 1536, |_, _| {
            let mut next = || {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                seed as u8
            };
            image::Rgba([next(), next(), next(), 255])
        });
        let mut encoded = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(picture)
            .write_to(&mut encoded, image::ImageFormat::Png)
            .unwrap();
        let raw = encoded.into_inner();
        assert!(raw.len() > 4 * 1024 * 1024 && raw.len() < INPUT_IMAGE_LIMIT);
        let value=inline(json!([{"type":"image_url","image_url":{"url":format!("data:image/png;base64,{}",STANDARD.encode(raw))}}])).await.unwrap();
        let url = value[0]["image_url"]["url"].as_str().unwrap();
        assert!(url.starts_with("data:image/jpeg;base64,"));
        let result = STANDARD.decode(url.split_once(',').unwrap().1).unwrap();
        assert!(result.len() < IMAGE_LIMIT);
        assert_eq!(image::load_from_memory(&result).unwrap().width(), 1536);
    }
}
