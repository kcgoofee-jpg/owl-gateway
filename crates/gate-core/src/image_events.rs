//! 图片事件流校验（`app/image_events.py`）：转发前逐条检查 SSE / msgpack 事件，只数「完整的最终图」。
//!
//! 规则（与旧版一致，都有快照测试）：
//! - 进度事件、EOF、传输字节数都不代表成功；只有合法的 `final`（样本序号在范围内、图片能 base64 解出且
//!   信封完整）才计数；同一个 `samp_ix` 重复只算一次；
//! - 显式 `error` 事件（`event_type == "error"` 或 payload 里有 `error` 键）让流失败，但此前已计数的 final 保留，
//!   且这条错误事件**不转发**；
//! - 任何格式问题（JSON / msgpack 解不开、事件类型不对、base64 / 信封不合法、超出大小限制）都立即拒绝：
//!   `failed = true`、缓冲清空、报 [`ImageStreamProtocolError`]（错误文案不含上游内容）；
//! - msgpack 帧 = 4 字节大端长度 + msgpack map；长度必须在 `1..=单事件上限`，声明长度先校验再缓冲；
//! - SSE 事件转发的是**重新编码**后的 `encode_sse(data, event)`，不是原始字节。
//!
//! 解析语义也与旧版一致：SSE 事件体走的是**默认的** `json.loads`（接受 `NaN` / `Infinity`、不限深度，约 1 万层才抛
//! `RecursionError`——见 `pyloads`），msgpack 走 `unpackb(raw=False)`（键只能是 str / bytes、str 必须是合法 UTF-8、
//! 容器嵌套 ≤ 1024，见 `pymsgpack`）；`base64.b64decode(validate=True)` 按 CPython 3.12.14 的严格模式（不同补丁版本
//! 对多余的 `=` 处理不同，所以这个版本号要和线上镜像保持一致）。
//!
//! 唯一的不可约差异在 `error_detail`（只进日志）：上游错误消息里的孤立代理换成 U+FFFD；消息本身是 list / dict / bytes
//! 且含生僻 Unicode 时，`repr` 里「是否可打印」只做了近似（未分配码位按可打印处理）。
//!
//! Python 的 `frames` 是生成器：路由逐帧转发、每帧后读 `completed_images`。这里一次返回所有帧，
//! 每帧附带「吐出这一帧时的已完成张数」，出错时错误排在已吐出的帧之后——调用方先转发帧、再处理错误即可。

use std::collections::HashSet;

use serde_json::Value;
use thiserror::Error;

use crate::pyc;
use crate::pyjson;
use crate::pyloads::{self, Options};
use crate::pymsgpack;
use crate::pyobj::Obj;
use crate::sse::{SseDecoder, encode_sse};

/// 单个事件上限。
pub const MAX_EVENT_BYTES: u64 = 32 * 1024 * 1024;
/// 单条流上限。
pub const MAX_STREAM_BYTES: u64 = 128 * 1024 * 1024;
/// 上游 error 事件原话进日志时的截断长度。
pub const ERROR_DETAIL_MAX_CHARS: usize = 120;

/// 线上格式。旧版里「不是 `msgpack` 的一律按 SSE 处理」。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WireFormat {
    Sse,
    Msgpack,
}

impl WireFormat {
    /// `STREAM_MEDIA_TYPES[wire_format]`
    pub fn media_type(self) -> &'static str {
        match self {
            WireFormat::Sse => "text/event-stream",
            WireFormat::Msgpack => "application/x-msgpack",
        }
    }
}

/// `ImageStreamProtocolError`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("NovelAI 返回的图片数据格式错误或过大，未完成的图未扣费。请稍后重试。")]
pub struct ImageStreamProtocolError;

/// `encode_image_event` 的失败。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum EncodeError {
    /// SSE 路径要用 `payload["event_type"].encode()`：缺失或不是字符串。
    #[error("payload 缺少字符串类型的 event_type")]
    NoEventType,
    /// msgpack 帧长度超过 4 字节能表示的范围。
    #[error("事件超过 4 GiB")]
    TooLarge,
}

/// `encode_image_event(payload, wire_format)`：msgpack = 4 字节大端长度 + `packb(payload, use_bin_type=True)`；
/// SSE = `encode_sse(json.dumps(payload, ensure_ascii=False), payload["event_type"])`。
pub fn encode_image_event(payload: &Value, wire: WireFormat) -> Result<Vec<u8>, EncodeError> {
    match wire {
        WireFormat::Msgpack => {
            let data = pymsgpack::packb(payload);
            let len = u32::try_from(data.len()).map_err(|_| EncodeError::TooLarge)?;
            let mut out = Vec::with_capacity(4 + data.len());
            out.extend_from_slice(&len.to_be_bytes());
            out.extend_from_slice(&data);
            Ok(out)
        }
        WireFormat::Sse => {
            let event = payload
                .get("event_type")
                .and_then(Value::as_str)
                .ok_or(EncodeError::NoEventType)?;
            Ok(encode_sse(
                pyjson::dumps(payload).as_bytes(),
                event.as_bytes(),
            ))
        }
    }
}

/// `_image_envelope`：只看格式与基本完整性，不解码像素。
pub fn image_envelope(data: &[u8]) -> bool {
    const PNG_SIG: &[u8] = b"\x89PNG\r\n\x1a\n";
    const PNG_IEND: &[u8] = b"\0\0\0\0IEND\xaeB`\x82";
    let be32 = |s: &[u8]| u32::from_be_bytes([s[0], s[1], s[2], s[3]]);
    if data.starts_with(PNG_SIG) {
        return data.len() >= 45
            && &data[8..16] == b"\0\0\0\rIHDR"
            && be32(&data[16..20]) > 0
            && be32(&data[20..24]) > 0
            && &data[data.len() - 12..] == PNG_IEND;
    }
    if data.starts_with(b"\xff\xd8\xff") {
        return data.len() >= 5 && data.ends_with(b"\xff\xd9");
    }
    if data.starts_with(b"RIFF") {
        // 先判长度再取字段（与旧版的求值顺序一致，短数据不会越界）
        if data.len() < 20 {
            return false;
        }
        let size = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
        return u64::from(size) == (data.len() - 8) as u64
            && &data[8..12] == b"WEBP"
            && matches!(&data[12..16], b"VP8 " | b"VP8L" | b"VP8X");
    }
    false
}

/// `base64.b64decode(s, validate=True)`（CPython 3.12.14 的 `binascii.a2b_base64(strict_mode=True)`）。
/// 失败（含非 ASCII 字符）返回 None。实现见 [`crate::policy::strict_b64`]。
pub fn b64decode_strict(s: &str) -> Option<Vec<u8>> {
    crate::policy::strict_b64(s, usize::MAX).map(|(v, _)| v)
}

/// `frames` 吐出的一帧：要转发给客户端的字节，以及吐出它时的已完成张数
/// （路由在每帧后读 `tracker.completed_images`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub bytes: Vec<u8>,
    pub completed_after: usize,
}

/// 一次 `frames` 调用的结果：先转发 `frames`，再处理 `error`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frames {
    pub frames: Vec<Frame>,
    pub error: Option<ImageStreamProtocolError>,
}

/// `ImageEventTracker.__init__` 的参数校验失败（`expected_images` 必须是正整数）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("expected_images must be a positive integer")]
pub struct InvalidExpectedImages;

/// 内部：校验失败的标记，由 `frames` 统一转成 `_reject()`。
struct Reject;

#[derive(Debug)]
pub struct ImageEventTracker {
    expected_images: i64,
    wire: WireFormat,
    max_event: u64,
    max_stream: u64,
    failed: bool,
    error_detail: String,
    completed: HashSet<i64>,
    first_image: Option<Vec<u8>>,
    sse: SseDecoder,
    binary: Vec<u8>,
    total_bytes: u64,
    closed: bool,
}

impl ImageEventTracker {
    /// 线上用的构造：单事件 32 MB、整条流 128 MB。
    pub fn new(expected_images: i64, wire: WireFormat) -> Result<Self, InvalidExpectedImages> {
        Self::with_limits(expected_images, wire, MAX_EVENT_BYTES, MAX_STREAM_BYTES)
    }

    /// 可调限额的构造（测试里用小限额验证边界；线上别用）。
    pub fn with_limits(
        expected_images: i64,
        wire: WireFormat,
        max_event: u64,
        max_stream: u64,
    ) -> Result<Self, InvalidExpectedImages> {
        if expected_images < 1 {
            return Err(InvalidExpectedImages);
        }
        Ok(Self {
            expected_images,
            wire,
            max_event,
            max_stream,
            failed: false,
            error_detail: String::new(),
            completed: HashSet::new(),
            first_image: None,
            sse: SseDecoder::new(max_event, max_stream),
            binary: Vec::new(),
            total_bytes: 0,
            closed: false,
        })
    }

    pub fn failed(&self) -> bool {
        self.failed
    }

    /// 上游 error 事件的原话（压缩空白、截断到 120 个字符），只进日志，不给客户端。
    pub fn error_detail(&self) -> &str {
        &self.error_detail
    }

    pub fn completed_images(&self) -> usize {
        self.completed.len()
    }

    /// 第一张完整的最终图（原始字节），供生成记录保存。
    pub fn first_image(&self) -> Option<&[u8]> {
        self.first_image.as_deref()
    }

    fn reject(&mut self) -> ImageStreamProtocolError {
        self.failed = true;
        self.finish();
        ImageStreamProtocolError
    }

    /// 流结束：丢弃没写完的数据（已计数的 final 保留）；之后再喂数据一律拒绝。
    pub fn finish(&mut self) {
        self.sse.finish();
        self.binary.clear();
        self.closed = true;
    }

    /// `frames`：校验一块数据，返回可转发的帧。
    pub fn frames(&mut self, chunk: &[u8]) -> Frames {
        let mut frames = Vec::new();
        let error = self.frames_into(chunk, &mut frames).err();
        Frames { frames, error }
    }

    fn frames_into(
        &mut self,
        chunk: &[u8],
        frames: &mut Vec<Frame>,
    ) -> Result<(), ImageStreamProtocolError> {
        if self.closed {
            return Err(self.reject());
        }
        if self.failed {
            return Ok(());
        }
        self.total_bytes += chunk.len() as u64;
        if self.total_bytes > self.max_stream {
            return Err(self.reject());
        }
        let result = match self.wire {
            WireFormat::Msgpack => self.frames_msgpack(chunk, frames),
            WireFormat::Sse => self.frames_sse(chunk, frames),
        };
        result.map_err(|Reject| self.reject())
    }

    fn header_size(&self) -> u64 {
        u64::from(u32::from_be_bytes([
            self.binary[0],
            self.binary[1],
            self.binary[2],
            self.binary[3],
        ]))
    }

    fn frames_msgpack(&mut self, chunk: &[u8], frames: &mut Vec<Frame>) -> Result<(), Reject> {
        let mut start = 0;
        while start < chunk.len() {
            let mut target = 4usize;
            if self.binary.len() >= 4 {
                let size = self.header_size();
                if !(0 < size && size <= self.max_event) {
                    return Err(Reject);
                }
                target += size as usize;
            }
            let take = (target - self.binary.len()).min(chunk.len() - start);
            self.binary.extend_from_slice(&chunk[start..start + take]);
            start += take;
            if self.binary.len() == target && target > 4 {
                let wire = std::mem::take(&mut self.binary);
                let payload = pymsgpack::unpackb(&wire[4..]).map_err(|_| Reject)?;
                self.dispatch(&payload, "")?;
                if self.failed {
                    return Ok(());
                }
                frames.push(Frame {
                    bytes: wire,
                    completed_after: self.completed.len(),
                });
            }
        }
        if self.binary.len() == 4 {
            let size = self.header_size();
            if !(0 < size && size <= self.max_event) {
                return Err(Reject);
            }
        }
        Ok(())
    }

    fn frames_sse(&mut self, chunk: &[u8], frames: &mut Vec<Frame>) -> Result<(), Reject> {
        let feed = self.sse.feed(chunk);
        for ev in feed.events {
            let text = std::str::from_utf8(&ev.data).map_err(|_| Reject)?;
            let payload = pyloads::loads_str(text, Options::LOADS)
                .map_err(|_| Reject)?
                .value;
            let name = std::str::from_utf8(&ev.event).map_err(|_| Reject)?;
            self.dispatch(&payload, name)?;
            if self.failed {
                return Ok(());
            }
            frames.push(Frame {
                bytes: encode_sse(&ev.data, &ev.event),
                completed_after: self.completed.len(),
            });
        }
        // 解码器在本块后半段超限：先前的事件已经处理完，这时才报错
        if feed.error.is_some() {
            return Err(Reject);
        }
        Ok(())
    }

    fn dispatch(&mut self, payload: &Obj, event_name: &str) -> Result<(), Reject> {
        let Obj::Dict(d) = payload else {
            return Err(Reject);
        };
        // payload.get("event_type", event_name)
        let event_type: &str = match d.get("event_type") {
            Some(Obj::Str(s)) => s,
            Some(_) => return Err(Reject),
            None => event_name,
        };
        if !(event_name.is_empty() || event_name == "message" || event_name == event_type) {
            return Err(Reject);
        }
        if event_type == "error" || d.contains("error") {
            self.failed = true;
            let msg = ["message", "error", "statusCode"]
                .iter()
                .filter_map(|k| d.get(k))
                .find(|v| v.truthy())
                .map(Obj::py_str)
                .unwrap_or_default();
            let squashed = msg
                .split(pyc::is_space)
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            self.error_detail = pyc::take_chars(&squashed, ERROR_DETAIL_MAX_CHARS).to_string();
            return Ok(());
        }
        if event_type != "final" {
            return Ok(());
        }
        // type(sample) is int and 0 <= sample < expected_images
        let sample = match d.get("samp_ix") {
            Some(Obj::Int(i)) if (0..i128::from(self.expected_images)).contains(i) => *i as i64,
            _ => return Err(Reject),
        };
        let decoded = match d.get("image") {
            Some(Obj::Str(s)) if !s.is_empty() => b64decode_strict(s).ok_or(Reject)?,
            Some(Obj::Bytes(b)) if !b.is_empty() => b.clone(),
            _ => return Err(Reject),
        };
        if !image_envelope(&decoded) {
            return Err(Reject);
        }
        self.completed.insert(sample);
        if self.first_image.is_none() {
            self.first_image = Some(decoded);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn png() -> Vec<u8> {
        let mut v = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        v.extend_from_slice(&[0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0, 0, 0, 0, 0]);
        v.extend_from_slice(b"\0\0\0\0IEND\xaeB`\x82");
        v
    }

    fn b64(data: &[u8]) -> String {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(data)
    }

    fn sse_final(sample: i64) -> Vec<u8> {
        let payload = json!({"event_type": "final", "samp_ix": sample, "image": b64(&png())});
        encode_sse(pyjson::dumps(&payload).as_bytes(), b"final")
    }

    #[test]
    fn envelope() {
        assert!(image_envelope(&png()));
        assert!(!image_envelope(&png()[..44]));
        assert!(image_envelope(b"\xff\xd8\xff\x00\xff\xd9"));
        assert!(!image_envelope(b"\xff\xd8\xff\x00"));
        assert!(image_envelope(b"RIFF\x0c\0\0\0WEBPVP8L\0\0\0\0"));
        assert!(!image_envelope(b"RIFF\xff\xff\xff\xffWEBPVP8L\0\0\0\0"));
    }

    #[test]
    fn strict_base64() {
        assert_eq!(b64decode_strict("AAA="), Some(vec![0, 0]));
        assert_eq!(b64decode_strict("AAAA="), None);
        assert_eq!(b64decode_strict("AA\nAA"), None);
        assert_eq!(b64decode_strict("=AAA"), None);
        assert_eq!(b64decode_strict("é"), None);
        assert_eq!(b64decode_strict(""), Some(vec![]));
    }

    #[test]
    fn sse_final_is_counted_once() {
        let mut t = ImageEventTracker::new(2, WireFormat::Sse).unwrap();
        let out = t.frames(&[sse_final(1), sse_final(1)].concat());
        assert_eq!(out.error, None);
        assert_eq!(out.frames.len(), 2);
        assert_eq!(out.frames[0].completed_after, 1);
        assert_eq!(t.completed_images(), 1);
        assert_eq!(t.first_image(), Some(&png()[..]));
    }

    #[test]
    fn error_event_keeps_finals_and_is_not_forwarded() {
        let mut t = ImageEventTracker::new(2, WireFormat::Sse).unwrap();
        let mut stream = sse_final(0);
        stream.extend_from_slice(b"event: error\ndata: {\"message\":\" up  stream\\nboom \"}\n\n");
        stream.extend_from_slice(&sse_final(1));
        let out = t.frames(&stream);
        assert_eq!(out.frames.len(), 1);
        assert!(t.failed() && t.completed_images() == 1);
        assert_eq!(t.error_detail(), "up stream boom");
    }

    #[test]
    fn malformed_is_rejected_and_closes() {
        let mut t = ImageEventTracker::new(1, WireFormat::Sse).unwrap();
        let out = t.frames(b"data: not json\n\n");
        assert_eq!(out.error, Some(ImageStreamProtocolError));
        assert!(t.failed());
        // 之后再喂也报错（缓冲已关闭）
        assert!(t.frames(b"x").error.is_some());
    }

    #[test]
    fn msgpack_framing_and_limits() {
        let payload = json!({"event_type": "final", "samp_ix": 0, "image": b64(&png())});
        let wire = encode_image_event(&payload, WireFormat::Msgpack).unwrap();
        let mut t = ImageEventTracker::new(1, WireFormat::Msgpack).unwrap();
        let (a, b) = wire.split_at(7);
        assert!(t.frames(a).frames.is_empty());
        let out = t.frames(b);
        assert_eq!(out.frames.len(), 1);
        assert_eq!(out.frames[0].bytes, wire);
        // 声明长度为 0 / 超限
        let mut t = ImageEventTracker::new(1, WireFormat::Msgpack).unwrap();
        assert!(t.frames(&[0, 0, 0, 0]).error.is_some());
        let mut t = ImageEventTracker::with_limits(1, WireFormat::Msgpack, 10, 100).unwrap();
        assert!(t.frames(&[0, 0, 0, 11]).error.is_some());
    }
}
