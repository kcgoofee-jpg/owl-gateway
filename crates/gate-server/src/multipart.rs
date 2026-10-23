//! Launcher 的 multipart 图片请求体：`app/body.py::read_bounded_body` + `app/image_payload.py::read_image_body`。
//!
//! Launcher 把二进制图片放在前面的附件 part（`image` / `mask` / `reference_image` / `ref_multiple_N` /
//! `director_ref_N`），最后一个名为 `request` 的 part 是 JSON，里面的图片字段写的是附件名。
//! 这里把附件展开成 Base64 填回 JSON，之后走和纯 JSON 请求一样的准入、参数与费用检查。
//!
//! 两个入口：
//! - [`read_bounded_body`]：先看 `Content-Length` 再边读边数，超限 413；
//! - [`parse_image_multipart`]：已读好的 multipart 体 → 展开后的 JSON 对象。
//!
//! 报错文案与状态码是客户端依赖的固定契约，不要随手改。解析层的取舍：
//! - multipart 的切分用 `multer`；`Content-Type` / `Content-Disposition` 的参数解析是
//!   `python_multipart.parse_options_header` 的逐行移植（大小写、引号转义、`name*=` 被忽略、`filename=` 即算文件），
//!   不用 multer 自带的（它对这些的取舍不同）；
//! - Starlette 的限额：文件 part ≤ 19（`REFERENCE_LIMIT + 3`），非文件 part ≤ 10，
//!   非文件 part 单个 ≤ 1 MiB；python-multipart 0.0.32 的单 part 头数量 ≤ 8、单个头 ≤ 4224 字节、边界 ≤ 256 字节。
//!   超出一律是 400「multipart 请求格式无效或过大」；
//! - `Content-Type` 主类型不是（小写的）`multipart/form-data` 时，Starlette 把表单当空的，
//!   所以得到 400「缺少 request JSON 字段」。
//!
//! 已知的行为边界（都只出现在手工构造的畸形体里，正常客户端不会碰到）：
//! - 带 `charset=` 的 `Content-Type`：只认 UTF-8 别名，其他字符集一律按 latin-1 解码；
//! - 体在最后一个 part 中途被截断 / 缺结尾边界：400「格式无效」，不静默丢 part；
//! - 边界后面的传输层空白、边界行里的前导杂字节、数据里出现 `\r\n--边界` 加其他字符：
//!   首个边界之前的杂字节与其后的空白按宽松方式处理（见 `start_check`），其余交给 multer；
//! - 单 part 的头数量按 multer 去重后的头计数，头大小按 `Name: value` 估算；
//! - JSON 里无法无损装进 `serde_json::Value` 的数字 / 孤立代理按「不是合法 JSON」拒绝（见 `gate_core::json_body::Lossy`）。

use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::future::Future;
use std::task::{Context, Poll, Waker};

use axum::body::Body;
use axum::http::HeaderMap;
use axum::http::header::{CONTENT_DISPOSITION, CONTENT_LENGTH};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use bytes::{Bytes, BytesMut};
use futures::StreamExt;
use gate_core::json_body::parse_json;
use gate_core::policy::REFERENCE_LIMIT;
use serde_json::{Map, Value};

use crate::error::HttpError;

const MSG_BAD_LENGTH: &str =
    "请求不完整，未处理。请重试或更换客户端。开发者参考：Content-Length 无效";
const MSG_TOO_LARGE: &str = "请求内容过大，未处理。请缩小图片或减少参考图。";
pub(crate) const MSG_INTERRUPTED: &str = "上传中断，未处理。请检查网络后重试。";

const MSG_BAD_MULTIPART: &str =
    "上传格式错误或内容过大，未扣费。请更新或更换客户端。开发者参考：multipart 请求格式无效或过大";
const MSG_DUPLICATE_FIELD: &str =
    "上传格式错误，未扣费。请更新或更换客户端。开发者参考：multipart 字段不能重名";
const MSG_NO_REQUEST: &str =
    "上传格式错误，未扣费。请更新或更换客户端。开发者参考：multipart 请求缺少 request JSON 字段";
const MSG_REQUEST_NOT_JSON: &str =
    "上传格式错误，未扣费。请更新或更换客户端。开发者参考：multipart 的 request 字段不是合法 JSON";
const MSG_REQUEST_NOT_OBJECT: &str = "上传格式错误，未扣费。请更新或更换客户端。开发者参考：multipart 的 request 字段必须是 JSON 对象";
const MSG_BAD_ATTACHMENT_FIELD: &str =
    "上传格式错误，未扣费。请更新或更换客户端。开发者参考：multipart 包含不支持的图片附件字段";
const MSG_MISSING_ATTACHMENT: &str =
    "上传格式错误，未扣费。请更新或更换客户端。开发者参考：multipart 缺少图片字段引用的附件";
const MSG_EXPANDED_TOO_LARGE: &str = "上传图片总大小超限，未扣费。请缩小图片或减少参考图。";
const MSG_EMPTY_ATTACHMENT: &str = "上传的图片中有空文件，未扣费。请重新添加图片。";
const MSG_TOO_MANY_REFERENCES: &str = "参考图过多，每次最多 16 张，未扣费。请减少参考图。";
const MSG_UNUSED_ATTACHMENT: &str =
    "上传格式错误，未扣费。请更新或更换客户端。开发者参考：multipart 包含未被图片字段引用的附件";

/// `Request.form(max_files=REFERENCE_LIMIT + 3, max_fields=10)`
const MAX_FILES: usize = REFERENCE_LIMIT + 3;
const MAX_FIELDS: usize = 10;
/// Starlette `MultiPartParser.max_part_size`：只约束没有 filename 的 part。
const MAX_FIELD_PART_BYTES: usize = 1024 * 1024;
/// python-multipart `DEFAULT_MAX_HEADER_COUNT` / `DEFAULT_MAX_HEADER_SIZE` / `MAX_BOUNDARY_LENGTH`
const MAX_PART_HEADERS: usize = 8;
const MAX_PART_HEADER_BYTES: usize = 4096 + 128;
const MAX_BOUNDARY_LEN: usize = 256;

fn bad_request(detail: &'static str) -> HttpError {
    HttpError::new(400, detail)
}

// ---------------------------------------------------------------------------------------------
// read_bounded_body
// ---------------------------------------------------------------------------------------------

/// `app/body.py::read_bounded_body`：先看 `Content-Length`（只认 ASCII 十进制数字，否则 400「Content-Length 无效」；
/// 超限（含数值溢出）413「请求体过大」），再读取请求体流，累计超限同样 413；
/// 读取中途客户端断开 → 400「请求体传输中断」。
pub async fn read_bounded_body(
    headers: &HeaderMap,
    body: Body,
    limit: usize,
) -> Result<Bytes, HttpError> {
    if let Some(declared) = headers.get(CONTENT_LENGTH) {
        check_declared_length(declared.as_bytes(), limit)?;
    }
    let mut stream = body.into_data_stream();
    let mut buf = BytesMut::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| bad_request(MSG_INTERRUPTED))?;
        // 读流失败几乎都是连接中断，一律 400。请求体是手动 into_data_stream 读的，
        // 没有 RequestBodyLimit 一类的外层包装，所以没有长度超限的 413 分支；以后加了这类层，要在这里补回来。
        if buf.len() + chunk.len() > limit {
            return Err(HttpError::new(413, MSG_TOO_LARGE));
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf.freeze())
}

fn check_declared_length(declared: &[u8], limit: usize) -> Result<(), HttpError> {
    // `declared.isascii() and declared.isdecimal()`；空串不是十进制数
    if declared.is_empty() || !declared.iter().all(u8::is_ascii_digit) {
        return Err(bad_request(MSG_BAD_LENGTH));
    }
    // 全是 ASCII 数字：解析失败只可能是溢出 u64，一样是超限；前导零照常解析
    let value = std::str::from_utf8(declared)
        .ok()
        .and_then(|d| d.parse::<u64>().ok());
    if value.is_none_or(|v| v > limit as u64) {
        return Err(HttpError::new(413, MSG_TOO_LARGE));
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// parse_image_multipart
// ---------------------------------------------------------------------------------------------

/// `app/image_payload.py::read_image_body` 的 multipart 分支（体已经用 [`read_bounded_body`] 读完）。
///
/// 返回 `request` part 里的 JSON 对象，其中指向附件的图片字段已替换成标准 Base64（带 `=` 填充）。
/// `limit` 与读体时用的相同，展开后的总大小也不能超过它。
///
/// 这是同步函数：JSON 解析与 Base64 编码在体接近上限时会占用可观的 CPU，调用方应放进 `spawn_blocking`。
pub fn parse_image_multipart(
    content_type: &str,
    body: Bytes,
    limit: usize,
) -> Result<Map<String, Value>, HttpError> {
    let items =
        parse_form(content_type.as_bytes(), body).map_err(|()| bad_request(MSG_BAD_MULTIPART))?;

    let mut names = HashSet::with_capacity(items.len());
    if !items.iter().all(|item| names.insert(item.name.as_str())) {
        return Err(bad_request(MSG_DUPLICATE_FIELD));
    }

    let raw: Bytes = match items.iter().find(|item| item.name == "request") {
        Some(Item {
            content: Content::File(bytes),
            ..
        }) => bytes.clone(),
        Some(Item {
            content: Content::Text(text),
            ..
        }) => Bytes::from(text.clone().into_bytes()),
        None => return Err(bad_request(MSG_NO_REQUEST)),
    };
    let parsed = parse_json(&raw).map_err(|_| bad_request(MSG_REQUEST_NOT_JSON))?;
    if parsed.lossy.any() {
        return Err(bad_request(MSG_REQUEST_NOT_JSON));
    }
    let Value::Object(mut data) = parsed.value else {
        return Err(bad_request(MSG_REQUEST_NOT_OBJECT));
    };

    let mut attachments: HashMap<&str, &Bytes> = HashMap::new();
    for item in items.iter().filter(|item| item.name != "request") {
        match &item.content {
            Content::File(bytes) if is_part_name(&item.name) => {
                attachments.insert(item.name.as_str(), bytes);
            }
            _ => return Err(bad_request(MSG_BAD_ATTACHMENT_FIELD)),
        }
    }

    let mut resolver = Resolver {
        attachments,
        used: HashSet::new(),
        encoded: HashMap::new(),
        expanded_size: raw.len() as i128,
        limit: limit as i128,
    };
    resolver.resolve_payload(&mut data)?;
    if resolver.used.len() != resolver.attachments.len() {
        return Err(bad_request(MSG_UNUSED_ATTACHMENT));
    }
    Ok(data)
}

/// `_PART_NAME.fullmatch`：
/// `(?:image|mask|reference_image|ref_multiple_[0-9]+|director_ref_[0-9]+)`
fn is_part_name(name: &str) -> bool {
    if matches!(name, "image" | "mask" | "reference_image") {
        return true;
    }
    ["ref_multiple_", "director_ref_"].iter().any(|prefix| {
        name.strip_prefix(prefix)
            .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()))
    })
}

struct Resolver<'a> {
    attachments: HashMap<&'a str, &'a Bytes>,
    used: HashSet<&'a str>,
    encoded: HashMap<&'a str, String>,
    /// `expanded_size`：每次解析一个指针就加上「Base64 长度 − 指针长度」，可能中途为负，所以用有符号数。
    expanded_size: i128,
    limit: i128,
}

impl<'a> Resolver<'a> {
    /// 只处理图片槽位：顶层对象和 `parameters`（若是对象）里的 `image` / `mask` / `reference_image`，
    /// 以及两个参考图列表里每个对象的 `data`。不碰任意字符串、JSON 路径或文件名。
    fn resolve_payload(&mut self, data: &mut Map<String, Value>) -> Result<(), HttpError> {
        let has_parameters = matches!(data.get("parameters"), Some(Value::Object(_)));
        self.resolve_container(data)?;
        if has_parameters && let Some(Value::Object(parameters)) = data.get_mut("parameters") {
            self.resolve_container(parameters)?;
        }
        Ok(())
    }

    fn resolve_container(&mut self, container: &mut Map<String, Value>) -> Result<(), HttpError> {
        for field in ["image", "mask", "reference_image"] {
            self.resolve(container, field)?;
        }
        for field in [
            "reference_image_multiple_cached",
            "director_reference_images_cached",
        ] {
            if let Some(Value::Array(values)) = container.get_mut(field) {
                if values.len() > REFERENCE_LIMIT {
                    return Err(bad_request(MSG_TOO_MANY_REFERENCES));
                }
                for item in values {
                    if let Value::Object(item) = item {
                        self.resolve(item, "data")?;
                    }
                }
            }
        }
        Ok(())
    }

    fn resolve(
        &mut self,
        container: &mut Map<String, Value>,
        field: &str,
    ) -> Result<(), HttpError> {
        let pointer = match container.get(field) {
            Some(Value::String(value)) if !value.is_empty() => value.clone(),
            _ => return Ok(()),
        };
        let Some((&name, &upload)) = self.attachments.get_key_value(pointer.as_str()) else {
            if is_part_name(&pointer) {
                return Err(bad_request(MSG_MISSING_ATTACHMENT));
            }
            return Ok(()); // 已经是内联 Base64：交给路由的参数校验
        };
        let encoded_len = 4 * upload.len().div_ceil(3);
        self.expanded_size += encoded_len as i128 - pointer.len() as i128;
        if self.expanded_size > self.limit {
            return Err(HttpError::new(413, MSG_EXPANDED_TOO_LARGE));
        }
        if !self.encoded.contains_key(name) {
            let encoded = STANDARD.encode(upload);
            if encoded.is_empty() {
                return Err(bad_request(MSG_EMPTY_ATTACHMENT));
            }
            self.encoded.insert(name, encoded);
        }
        container.insert(field.to_owned(), Value::String(self.encoded[name].clone()));
        self.used.insert(name);
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------
// 表单解析（Starlette `Request.form` 的等价物）
// ---------------------------------------------------------------------------------------------

enum Content {
    /// 带 filename 的 part（Starlette 的 `UploadFile`）
    File(Bytes),
    /// 不带 filename 的 part，按字符集解码成字符串
    Text(String),
}

struct Item {
    name: String,
    content: Content,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Charset {
    Utf8,
    Latin1,
}

/// 解析失败（本层任何一步的失败都归到这里）。
type FormResult<T> = Result<T, ()>;

fn parse_form(content_type: &[u8], body: Bytes) -> FormResult<Vec<Item>> {
    let (media_type, params) = parse_options_header(content_type);
    if media_type != b"multipart/form-data" {
        return Ok(Vec::new()); // Starlette：既不是 multipart 也不是 urlencoded → 空表单
    }
    let charset = charset_of(params.get(b"charset".as_slice()).map(Vec::as_slice));
    let boundary = params.get(b"boundary".as_slice()).ok_or(())?;
    if boundary.len() > MAX_BOUNDARY_LEN {
        return Err(());
    }
    let body = match start_check(&body, boundary) {
        Start::Empty => return Ok(Vec::new()),
        Start::Bad => return Err(()),
        Start::At(offset) => body.slice(offset..),
    };
    let boundary = String::from_utf8(boundary.clone()).map_err(|_| ())?;
    // 体已完整在内存里，multer 的流只有一个就绪的块：一次 poll 必然跑完，不需要执行器。
    poll_ready(collect_items(boundary, body, charset)).unwrap_or(Err(()))
}

async fn collect_items(boundary: String, body: Bytes, charset: Charset) -> FormResult<Vec<Item>> {
    let stream = futures::stream::once(std::future::ready(Ok::<Bytes, Infallible>(body)));
    let mut multipart = multer::Multipart::new(stream, boundary);
    let (mut files, mut fields) = (0usize, 0usize);
    let mut items = Vec::new();
    while let Some(mut field) = multipart.next_field().await.map_err(|_| ())? {
        // 头大小按常见写法 `Name: value` 估算
        let oversized = |(name, value): (&axum::http::HeaderName, &axum::http::HeaderValue)| {
            name.as_str().len() + 2 + value.len() > MAX_PART_HEADER_BYTES
        };
        if field.headers().len() > MAX_PART_HEADERS || field.headers().iter().any(oversized) {
            return Err(());
        }
        let disposition = field
            .headers()
            .get(CONTENT_DISPOSITION)
            .map_or(&[][..], |v| v.as_bytes());
        let (_, options) = parse_options_header(disposition);
        let name = options.get(b"name".as_slice()).ok_or(())?;
        let name = user_safe_decode(name, charset);
        let is_file = options.contains_key(b"filename".as_slice());
        if is_file {
            files += 1;
            if files > MAX_FILES {
                return Err(());
            }
        } else {
            fields += 1;
            if fields > MAX_FIELDS {
                return Err(());
            }
        }
        let mut data = BytesMut::new();
        loop {
            match field.chunk().await {
                Ok(Some(chunk)) => {
                    if !is_file && data.len() + chunk.len() > MAX_FIELD_PART_BYTES {
                        return Err(());
                    }
                    data.extend_from_slice(&chunk);
                }
                Ok(None) => break,
                // 头部完整、数据一直读到体结束也没等到下一个边界：python-multipart 不报错，
                // 只是这个 part 永远没有「结束」事件，于是被丢弃，前面已结束的 part 照常使用。
                Err(multer::Error::IncompleteFieldData { .. }) => return Ok(items),
                Err(_) => return Err(()),
            }
        }
        let content = if is_file {
            Content::File(data.freeze())
        } else {
            Content::Text(user_safe_decode(&data, charset))
        };
        items.push(Item { name, content });
    }
    Ok(items)
}

/// 只 poll 一次；`Pending` 视为失败（不会发生，见 `parse_form`）。
fn poll_ready<F: Future>(future: F) -> Option<F::Output> {
    let mut future = std::pin::pin!(future);
    match future
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
    {
        Poll::Ready(out) => Some(out),
        Poll::Pending => None,
    }
}

enum Start {
    /// 体里一个 part 都没有
    Empty,
    /// 开头不合法
    Bad,
    /// 第一个边界从这个偏移开始
    At(usize),
}

/// python-multipart `MultipartParser` 开头的状态机：
/// 先跳过开头的 CR / LF（并一路跳到下一个 `-`），之后必须是 `--边界`，再紧跟 CRLF（新 part）或 `--`（空消息）。
/// 不允许前导杂文字，也不允许边界行尾有空白（multer 对这两处更宽松）。体在边界中途结束不算错，只是没有 part。
fn start_check(body: &[u8], boundary: &[u8]) -> Start {
    let Some(&first) = body.first() else {
        return Start::Empty;
    };
    let mut begin = 0;
    if first == b'\r' || first == b'\n' {
        match body.iter().position(|&b| b == b'-') {
            Some(dash) => begin = dash,
            None => return Start::Empty,
        }
    }
    let rest = &body[begin..];
    let delimiter = [b"--".as_slice(), boundary].concat();
    let common = rest.len().min(delimiter.len());
    if rest[..common] != delimiter[..common] {
        return Start::Bad;
    }
    if rest.len() <= delimiter.len() {
        return Start::Empty;
    }
    let after = &rest[delimiter.len()..];
    match (after[0], after.get(1)) {
        (b'\r', Some(b'\n')) | (b'-', Some(b'-')) => Start::At(begin),
        (b'\r' | b'-', None) => Start::Empty,
        _ => Start::Bad,
    }
}

/// 字符集参数：只区分 UTF-8 别名；未知字符集按 latin-1 解码。
fn charset_of(param: Option<&[u8]>) -> Charset {
    let Some(raw) = param else {
        return Charset::Utf8;
    };
    // `encodings.normalize_encoding`：非字母数字（点除外）折叠成一个下划线，去掉首尾下划线
    let mut name = String::new();
    for &b in raw {
        if b.is_ascii_alphanumeric() || b == b'.' {
            name.push(b.to_ascii_lowercase() as char);
        } else if !name.ends_with('_') {
            name.push('_');
        }
    }
    match name.trim_matches('_') {
        "utf_8" | "utf8" | "u8" | "utf" | "utf8_ucs2" | "utf8_ucs4" | "cp65001" => Charset::Utf8,
        _ => Charset::Latin1,
    }
}

/// `starlette.formparsers._user_safe_decode`
fn user_safe_decode(src: &[u8], charset: Charset) -> String {
    if charset == Charset::Utf8
        && let Ok(text) = std::str::from_utf8(src)
    {
        return text.to_owned();
    }
    src.iter().map(|&b| char::from(b)).collect()
}

// ---------------------------------------------------------------------------------------------
// python_multipart.multipart.parse_options_header 的移植（按 latin-1 字节处理）
// ---------------------------------------------------------------------------------------------

type Options = HashMap<Vec<u8>, Vec<u8>>;

/// 返回 `(主值, 参数表)`。参数名转小写，同名取后者，值去掉引号并处理 `\\` / `\"` 转义；
/// 带 `*` 的参数名（RFC 5987 扩展语法）被忽略。
fn parse_options_header(value: &[u8]) -> (Vec<u8>, Options) {
    if value.is_empty() {
        return (Vec::new(), Options::new());
    }
    if !value.contains(&b';') {
        return (strip(&value.to_ascii_lowercase()).to_vec(), Options::new());
    }
    let mut segments = parse_params(value).into_iter();
    let media_type = segments.next().unwrap_or_default();
    let mut options = Options::new();
    for segment in segments {
        let (key, mut val) = match segment.iter().position(|&b| b == b'=') {
            Some(eq) => (segment[..eq].to_vec(), segment[eq + 1..].to_vec()),
            None => (segment.clone(), Vec::new()),
        };
        if key.contains(&b'*') {
            continue;
        }
        if val.len() >= 2 && val[0] == b'"' && val[val.len() - 1] == b'"' {
            let inner = &val[1..val.len() - 1];
            val = replace(&replace(inner, b"\\\\", b"\\"), b"\\\"", b"\"");
        }
        options.insert(key, val);
    }
    (media_type, options)
}

/// `_parseparam`（含它的逐字实现，包括负下标 / 空区间的 Python 切片语义）。
fn parse_params(value: &[u8]) -> Vec<Vec<u8>> {
    let mut s = Vec::with_capacity(value.len() + 1);
    s.push(b';');
    s.extend_from_slice(value);
    let mut params = Vec::new();
    let mut start: i64 = 0;
    while find(&s, b";", start, None) == start {
        start += 1;
        let mut end = find(&s, b";", start, None);
        let (mut ind, mut diff) = (start, 0i64);
        while end > 0 {
            diff += count(&s, b"\"", ind, end) - count(&s, b"\\\"", ind, end);
            if diff % 2 == 0 {
                break;
            }
            let next = find(&s, b";", end + 1, None);
            end = ind;
            ind = next;
        }
        if end < 0 {
            end = s.len() as i64;
        }
        let eq = find(&s, b"=", start, Some(end));
        let part = if eq == -1 {
            slice(&s, start, end).to_vec()
        } else {
            let mut joined = rstrip(slice(&s, start, eq)).to_ascii_lowercase();
            joined.push(b'=');
            joined.extend_from_slice(lstrip(slice(&s, eq + 1, end)));
            joined
        };
        params.push(strip(&part).to_vec());
        start = end;
    }
    params
}

/// Python `str.isspace()` 对 latin-1 字符的判定（比 `u8::is_ascii_whitespace` 多 VT、FS/GS/RS/US、NEL、NBSP）。
fn is_space(b: u8) -> bool {
    matches!(
        b,
        b'\t' | b'\n' | 0x0b | 0x0c | b'\r' | 0x1c..=0x1f | b' ' | 0x85 | 0xa0
    )
}

fn lstrip(s: &[u8]) -> &[u8] {
    let start = s.iter().position(|&b| !is_space(b)).unwrap_or(s.len());
    &s[start..]
}

fn rstrip(s: &[u8]) -> &[u8] {
    let end = s.iter().rposition(|&b| !is_space(b)).map_or(0, |i| i + 1);
    &s[..end]
}

fn strip(s: &[u8]) -> &[u8] {
    lstrip(rstrip(s))
}

/// Python 切片 `s[start:end]` 的下标规整：负数加长度（仍为负则取 0），超出长度取长度。
fn bounds(len: usize, start: i64, end: i64) -> (usize, usize) {
    let norm = |i: i64| -> usize {
        let i = if i < 0 { i + len as i64 } else { i };
        i.clamp(0, len as i64) as usize
    };
    (norm(start), norm(end))
}

fn slice(s: &[u8], start: i64, end: i64) -> &[u8] {
    let (a, b) = bounds(s.len(), start, end);
    if a >= b { &[] } else { &s[a..b] }
}

/// `s.find(sub, start[, end])`，找不到返回 -1。
fn find(s: &[u8], sub: &[u8], start: i64, end: Option<i64>) -> i64 {
    let (a, b) = bounds(s.len(), start, end.unwrap_or(s.len() as i64));
    if a > b || b - a < sub.len() {
        return -1;
    }
    s[a..b]
        .windows(sub.len())
        .position(|w| w == sub)
        .map_or(-1, |i| (a + i) as i64)
}

/// `s.count(sub, start, end)`（不重叠）。
fn count(s: &[u8], sub: &[u8], start: i64, end: i64) -> i64 {
    let (a, b) = bounds(s.len(), start, end);
    if a >= b {
        return 0;
    }
    let (mut n, mut i) = (0, a);
    while i + sub.len() <= b {
        if &s[i..i + sub.len()] == sub {
            n += 1;
            i += sub.len();
        } else {
            i += 1;
        }
    }
    n
}

/// `bytes.replace(old, new)`（`old` 非空）。
fn replace(s: &[u8], old: &[u8], new: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        if s[i..].starts_with(old) {
            out.extend_from_slice(new);
            i += old.len();
        } else {
            out.push(s[i]);
            i += 1;
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------
// 测试
//
// 期望值来自旧版：用 Python 3.12 + requirements.txt 解析出的依赖（starlette 1.8 / python-multipart 0.0.32）
// 直接调用 `app.image_payload.read_image_body` / `app.body.read_bounded_body` 得到。
// 体的构造方式与 httpx / 浏览器一致：`--边界` 行、`Content-Disposition`（文件 part 带 `filename="blob"`）、
// 可选 `Content-Type`、空行、数据，最后是 `--边界--`。
// ---------------------------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const BOUNDARY: &str = "owlbnd0123456789abcdef0123456789";
    const LIMIT: usize = 25 * 1024 * 1024;
    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\x00\x01\x02\x03";
    const JPG: &[u8] = b"\xff\xd8\xff\xe0\x00\x10JFIF";

    const E_FRAMING: &str = "上传格式错误或内容过大，未扣费。请更新或更换客户端。开发者参考：multipart 请求格式无效或过大";
    const E_DUPLICATE: &str =
        "上传格式错误，未扣费。请更新或更换客户端。开发者参考：multipart 字段不能重名";
    const E_NO_REQUEST: &str = "上传格式错误，未扣费。请更新或更换客户端。开发者参考：multipart 请求缺少 request JSON 字段";
    const E_NOT_JSON: &str = "上传格式错误，未扣费。请更新或更换客户端。开发者参考：multipart 的 request 字段不是合法 JSON";
    const E_NOT_OBJECT: &str = "上传格式错误，未扣费。请更新或更换客户端。开发者参考：multipart 的 request 字段必须是 JSON 对象";
    const E_BAD_FIELD: &str =
        "上传格式错误，未扣费。请更新或更换客户端。开发者参考：multipart 包含不支持的图片附件字段";
    const E_MISSING: &str =
        "上传格式错误，未扣费。请更新或更换客户端。开发者参考：multipart 缺少图片字段引用的附件";
    const E_EXPANDED: &str = "上传图片总大小超限，未扣费。请缩小图片或减少参考图。";
    const E_EMPTY: &str = "上传的图片中有空文件，未扣费。请重新添加图片。";
    const E_REFS: &str = "参考图过多，每次最多 16 张，未扣费。请减少参考图。";
    const E_UNUSED: &str = "上传格式错误，未扣费。请更新或更换客户端。开发者参考：multipart 包含未被图片字段引用的附件";

    type Parsed = Result<Map<String, Value>, HttpError>;

    struct Part {
        name: String,
        filename: Option<&'static str>,
        ctype: Option<&'static str>,
        data: Vec<u8>,
    }

    /// 带 `filename="blob"` 的图片附件（Launcher / httpx 的 Blob 上传）
    fn file(name: &str, data: &[u8]) -> Part {
        Part {
            name: name.to_owned(),
            filename: Some("blob"),
            ctype: Some("image/png"),
            data: data.to_vec(),
        }
    }

    /// 以文件形式上传的 request JSON
    fn req(payload: impl AsRef<[u8]>) -> Part {
        Part {
            name: "request".to_owned(),
            filename: Some("blob"),
            ctype: Some("application/json"),
            data: payload.as_ref().to_vec(),
        }
    }

    /// 没有 filename 的 request part（Starlette 当普通字段）
    fn req_text(payload: impl AsRef<[u8]>) -> Part {
        Part {
            filename: None,
            ..req(payload)
        }
    }

    fn field(name: &str, data: &str) -> Part {
        Part {
            name: name.to_owned(),
            filename: None,
            ctype: None,
            data: data.as_bytes().to_vec(),
        }
    }

    fn build_with(parts: &[Part], boundary: &str, closing: bool) -> Vec<u8> {
        let mut out = Vec::new();
        for part in parts {
            out.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
            out.extend_from_slice(
                format!("Content-Disposition: form-data; name=\"{}\"", part.name).as_bytes(),
            );
            if let Some(filename) = part.filename {
                out.extend_from_slice(format!("; filename=\"{filename}\"").as_bytes());
            }
            if let Some(ctype) = part.ctype {
                out.extend_from_slice(format!("\r\nContent-Type: {ctype}").as_bytes());
            }
            out.extend_from_slice(b"\r\n\r\n");
            out.extend_from_slice(&part.data);
            out.extend_from_slice(b"\r\n");
        }
        if closing {
            out.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
        }
        out
    }

    fn build(parts: &[Part]) -> Vec<u8> {
        build_with(parts, BOUNDARY, true)
    }

    /// 只有 `Content-Disposition` 头的 part（头部原样写入，用来测各种写法）
    fn raw_part(disposition: &str, data: &[u8]) -> Vec<u8> {
        let mut out =
            format!("--{BOUNDARY}\r\nContent-Disposition: {disposition}\r\n\r\n").into_bytes();
        out.extend_from_slice(data);
        out.extend_from_slice(b"\r\n");
        out
    }

    fn closing() -> Vec<u8> {
        format!("--{BOUNDARY}--\r\n").into_bytes()
    }

    fn content_type() -> String {
        format!("multipart/form-data; boundary={BOUNDARY}")
    }

    fn run(content_type: &str, body: &[u8], limit: usize) -> Parsed {
        parse_image_multipart(content_type, Bytes::copy_from_slice(body), limit)
    }

    fn parse(parts: &[Part]) -> Parsed {
        run(&content_type(), &build(parts), LIMIT)
    }

    fn ok_value(result: Parsed) -> Value {
        match result {
            Ok(map) => Value::Object(map),
            Err(e) => panic!("期望成功，实际 {} {}", e.status, e.detail),
        }
    }

    #[track_caller]
    fn expect_err(what: &str, result: Parsed, status: u16, detail: &str) {
        match result {
            Ok(map) => panic!(
                "{what}：期望 {status} {detail}，实际成功：{}",
                Value::Object(map)
            ),
            Err(e) => assert!(
                e.status == status && e.detail == json!(detail),
                "{what}：期望 {status} {detail}，实际 {} {}",
                e.status,
                e.detail
            ),
        }
    }

    #[track_caller]
    fn assert_400(result: Parsed, detail: &str) {
        expect_err("", result, 400, detail);
    }

    fn b64(data: &[u8]) -> String {
        STANDARD.encode(data)
    }

    // ---- 成功路径 -----------------------------------------------------------------------------

    #[test]
    fn happy_path_image_mask_and_shared_references() {
        let payload = json!({
            "input": "image",
            "image": "image",
            "parameters": {
                "image": "image",
                "mask": "mask",
                "reference_image": "ref_multiple_0",
                "reference_image_multiple_cached": [
                    {"cache_secret_key": "k0", "data": "ref_multiple_0"},
                    {"cache_secret_key": "k1", "data": "ref_multiple_0"},
                ],
                "director_reference_images_cached": [
                    {"cache_secret_key": "k2", "data": "director_ref_0"},
                    {"cache_secret_key": "k3", "data": "director_ref_0"},
                    "x",
                    {"data": "iVBORw0KGgo="},
                ],
                "negative_prompt": "mask",
                "extra": {"image": "image"},
            },
        });
        let got = ok_value(parse(&[
            file("image", PNG),
            file("mask", JPG),
            file("ref_multiple_0", b"ab"),
            file("director_ref_0", b"abc"),
            req(payload.to_string()),
        ]));
        // 旧版输出：同一指针只编码一次、各处共用；只展开图片槽位，其他字符串（input / negative_prompt /
        // 嵌套对象里的 image / 已经是 Base64 的 data）原样保留
        let expected = json!({
            "input": "image",
            "image": "iVBORw0KGgoAAQID",
            "parameters": {
                "image": "iVBORw0KGgoAAQID",
                "mask": "/9j/4AAQSkZJRg==",
                "reference_image": "YWI=",
                "reference_image_multiple_cached": [
                    {"cache_secret_key": "k0", "data": "YWI="},
                    {"cache_secret_key": "k1", "data": "YWI="},
                ],
                "director_reference_images_cached": [
                    {"cache_secret_key": "k2", "data": "YWJj"},
                    {"cache_secret_key": "k3", "data": "YWJj"},
                    "x",
                    {"data": "iVBORw0KGgo="},
                ],
                "negative_prompt": "mask",
                "extra": {"image": "image"},
            },
        });
        assert_eq!(got, expected);
        // 键顺序保持（Python dict 赋值不改变位置）
        let keys: Vec<_> = got["parameters"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        assert_eq!(
            keys,
            [
                "image",
                "mask",
                "reference_image",
                "reference_image_multiple_cached",
                "director_reference_images_cached",
                "negative_prompt",
                "extra"
            ]
        );
    }

    #[test]
    fn base64_is_standard_with_padding() {
        let got = ok_value(parse(&[
            file("image", b"a"),
            file("mask", b"ab"),
            file("reference_image", b"abc"),
            req(r#"{"image":"image","mask":"mask","reference_image":"reference_image"}"#),
        ]));
        assert_eq!(
            got,
            json!({"image": "YQ==", "mask": "YWI=", "reference_image": "YWJj"})
        );
        // 全部 256 个字节值：确认用的是 `+` `/` 字母表，不是 URL-safe
        let all: Vec<u8> = (0..=255).collect();
        let got = ok_value(parse(&[file("image", &all), req(r#"{"image":"image"}"#)]));
        assert_eq!(got["image"], json!(b64(&all)));
        assert!(got["image"].as_str().unwrap().contains('+'));
    }

    #[test]
    fn request_part_may_come_first_or_be_a_plain_field() {
        // request 在前也行
        let got = ok_value(parse(&[req(r#"{"image":"image"}"#), file("image", PNG)]));
        assert_eq!(got, json!({"image": "iVBORw0KGgoAAQID"}));
        // 没有 filename 的 request：Starlette 当字段，文本按 UTF-8 解码后再编码成 UTF-8 交给 JSON 解析
        let got = ok_value(parse(&[
            file("image", PNG),
            req_text(r#"{"image":"image","n":"é"}"#),
        ]));
        assert_eq!(got, json!({"image": "iVBORw0KGgoAAQID", "n": "é"}));
        // 字段里不是 UTF-8 的字节：旧版按 latin-1 兜底（0xE9 → é）
        let got = ok_value(parse(&[
            file("image", PNG),
            req_text(b"{\"n\":\"\xe9\",\"image\":\"image\"}"),
        ]));
        assert_eq!(got, json!({"n": "é", "image": "iVBORw0KGgoAAQID"}));
        // 同样的字节以文件形式上传：直接当 bytes 解析，非法 UTF-8 → 不是合法 JSON
        assert_400(
            parse(&[
                file("image", PNG),
                req(b"{\"n\":\"\xe9\",\"image\":\"image\"}"),
            ]),
            E_NOT_JSON,
        );
    }

    #[test]
    fn inline_base64_and_unknown_strings_are_left_alone() {
        let got = ok_value(parse(&[req(
            r#"{"image":"iVBORw0KGgo=","parameters":{"mask":"../x","reference_image":"Image"}}"#,
        )]));
        assert_eq!(
            got,
            json!({"image": "iVBORw0KGgo=", "parameters": {"mask": "../x", "reference_image": "Image"}})
        );
        // 空串 / 非字符串 / null 不是指针
        let odd = json!({"image": "", "mask": null, "reference_image": 5, "parameters": {"image": ["image"]}});
        assert_eq!(ok_value(parse(&[req(odd.to_string())])), odd);
    }

    #[test]
    fn parameters_that_is_not_an_object_is_skipped() {
        let got = ok_value(parse(&[
            file("image", PNG),
            req(r#"{"image":"image","parameters":["image"]}"#),
        ]));
        assert_eq!(
            got,
            json!({"image": "iVBORw0KGgoAAQID", "parameters": ["image"]})
        );
    }

    #[test]
    fn leading_blank_lines_and_boundary_length_limit() {
        // 开头的空行被跳过
        let mut body = b"\r\n".to_vec();
        body.extend(build(&[req("{}")]));
        assert_eq!(ok_value(run(&content_type(), &body, LIMIT)), json!({}));
        // python-multipart 的边界上限是 256 字节
        let long = "b".repeat(256);
        let ct = format!("multipart/form-data; boundary={long}");
        let body = build_with(&[req("{}")], &long, true);
        assert_eq!(ok_value(run(&ct, &body, LIMIT)), json!({}));
        let long = "b".repeat(257);
        let ct = format!("multipart/form-data; boundary={long}");
        let body = build_with(&[req("{}")], &long, true);
        assert_400(run(&ct, &body, LIMIT), E_FRAMING);
    }

    #[test]
    fn json_depth_limit_is_32() {
        let nested = |depth: usize| format!("{{\"a\":{}{}}}", "[".repeat(depth), "]".repeat(depth));
        assert!(parse(&[req(nested(31))]).is_ok()); // 加上外层对象正好 32 层
        assert_400(parse(&[req(nested(32))]), E_NOT_JSON);
    }

    // ---- 400「格式无效或过大」：Starlette 解析失败与限额 ------------------------------------------

    #[test]
    fn malformed_multipart_is_rejected_with_the_generic_message() {
        let ok_body = build(&[req("{}")]);
        let no_name = format!(
            "--{BOUNDARY}\r\nContent-Disposition: form-data\r\n\r\nx\r\n--{BOUNDARY}--\r\n"
        );
        let no_disposition =
            format!("--{BOUNDARY}\r\nContent-Type: text/plain\r\n\r\nx\r\n--{BOUNDARY}--\r\n");
        let cases: Vec<(&str, String, Vec<u8>)> = vec![
            (
                "缺少 boundary 参数",
                "multipart/form-data".into(),
                ok_body.clone(),
            ),
            (
                "只有 charset 没有 boundary",
                "multipart/form-data; charset=utf-8".into(),
                ok_body.clone(),
            ),
            (
                "边界与体不一致",
                "multipart/form-data; boundary=other".into(),
                ok_body.clone(),
            ),
            (
                "体不是 multipart",
                content_type(),
                b"not multipart at all".to_vec(),
            ),
            (
                "边界前有杂文字",
                content_type(),
                [b"preamble\r\n".as_slice(), &ok_body].concat(),
            ),
            ("part 缺少 name", content_type(), no_name.into_bytes()),
            (
                "part 没有 Content-Disposition",
                content_type(),
                no_disposition.into_bytes(),
            ),
        ];
        for (what, ct, body) in cases {
            expect_err(what, run(&ct, &body, LIMIT), 400, E_FRAMING);
        }
    }

    #[test]
    fn empty_or_unparsed_forms_report_the_missing_request() {
        // Starlette：既不是（小写的）multipart/form-data 也不是 urlencoded → 空表单
        let body = build(&[file("image", PNG), req("{}")]);
        for ct in [
            format!("Multipart/Form-Data; boundary={BOUNDARY}"),
            format!("multipart/mixed; boundary={BOUNDARY}"),
            String::new(),
        ] {
            expect_err(&ct, run(&ct, &body, LIMIT), 400, E_NO_REQUEST);
        }
        // 空体、只有换行、只有结束边界：没有任何 part
        assert_400(run(&content_type(), b"", LIMIT), E_NO_REQUEST);
        assert_400(run(&content_type(), b"\r\n\r\n", LIMIT), E_NO_REQUEST);
        assert_400(run(&content_type(), &closing(), LIMIT), E_NO_REQUEST);
        // 最后一个 part 没有收到结束边界：旧版静默丢弃它（于是 request 不见了）
        let unterminated = build_with(
            &[file("image", PNG), req(r#"{"image":"image"}"#)],
            BOUNDARY,
            false,
        );
        assert_400(run(&content_type(), &unterminated, LIMIT), E_NO_REQUEST);
        // 没有 request，只有别的字段 / 附件
        assert_400(parse(&[file("image", PNG)]), E_NO_REQUEST);
        assert_400(parse(&[field("extra", "v")]), E_NO_REQUEST);
    }

    #[test]
    fn starlette_file_and_field_limits() {
        let attachments = |n: usize| -> Vec<Part> {
            (0..n)
                .map(|i| file(&format!("ref_multiple_{i}"), b"a"))
                .collect()
        };
        // 18 个附件 + 文件形式的 request = 19 个文件：通过解析（之后因为没被引用而报错）
        let mut parts = attachments(18);
        parts.push(req("{}"));
        assert_400(parse(&parts), E_UNUSED);
        // 18 + image 附件 + 无 filename 的 request：仍是 19 个文件
        let mut parts = attachments(18);
        parts.push(file("image", b"a"));
        parts.push(req_text(r#"{"image":"image"}"#));
        assert_400(parse(&parts), E_UNUSED);
        // 第 20 个文件
        let mut parts = attachments(18);
        parts.push(file("image", b"a"));
        parts.push(req(r#"{"image":"image"}"#));
        assert_400(parse(&parts), E_FRAMING);
        let mut parts = attachments(20);
        parts.push(req_text("{}"));
        assert_400(parse(&parts), E_FRAMING);

        let extras =
            |n: usize| -> Vec<Part> { (0..n).map(|i| field(&format!("extra{i}"), "v")).collect() };
        // 10 个字段（9 个额外 + 无 filename 的 request）：通过解析（额外字段不是图片附件）
        let mut parts = extras(9);
        parts.push(req_text("{}"));
        assert_400(parse(&parts), E_BAD_FIELD);
        // 第 11 个字段
        let mut parts = extras(10);
        parts.push(req_text("{}"));
        assert_400(parse(&parts), E_FRAMING);
        // request 带 filename 时它算文件，不占字段名额
        let mut parts = extras(10);
        parts.push(req("{}"));
        assert_400(parse(&parts), E_BAD_FIELD);
        let mut parts = extras(11);
        parts.push(req("{}"));
        assert_400(parse(&parts), E_FRAMING);
    }

    #[test]
    fn non_file_parts_are_capped_at_one_mebibyte_but_files_are_not() {
        // `{"image":"xxx…"}` 补到正好 1 MiB
        let exact = format!("{{\"image\":\"{}\"}}", "x".repeat(1024 * 1024 - 12));
        assert_eq!(exact.len(), 1024 * 1024);
        assert!(parse(&[req_text(&exact)]).is_ok());
        let over = format!("{exact} ");
        assert_400(parse(&[req_text(&over)]), E_FRAMING);
        // 文件形式不受 1 MiB 限制
        assert!(parse(&[req(&over)]).is_ok());
    }

    #[test]
    fn part_header_count_is_capped_at_eight() {
        let part_with = |extra: usize| {
            let mut body = format!(
                "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"request\"; filename=\"b\"\r\n"
            );
            for i in 0..extra {
                body.push_str(&format!("X-H{i}: v\r\n"));
            }
            body.push_str(&format!("\r\n{{}}\r\n--{BOUNDARY}--\r\n"));
            body.into_bytes()
        };
        assert_eq!(
            ok_value(run(&content_type(), &part_with(7), LIMIT)),
            json!({})
        );
        assert_400(run(&content_type(), &part_with(8), LIMIT), E_FRAMING);
    }

    #[test]
    fn part_header_size_is_capped_at_4224_bytes() {
        let part_with = |value_len: usize| {
            format!(
                "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"request\"; filename=\"b\"\r\nX-Long: {}\r\n\r\n{{}}\r\n--{BOUNDARY}--\r\n",
                "v".repeat(value_len)
            )
            .into_bytes()
        };
        // 名字 6 + 冒号 1 + 空格 1 + 值：4216 字节正好 4224
        assert_eq!(
            ok_value(run(&content_type(), &part_with(4216), LIMIT)),
            json!({})
        );
        assert_400(run(&content_type(), &part_with(4217), LIMIT), E_FRAMING);
        assert_400(run(&content_type(), &part_with(5000), LIMIT), E_FRAMING);
        // Content-Disposition 本身同样受限
        let long_name = |n: usize| {
            let disposition = format!(
                "form-data; name=\"request\"; filename=\"{}\"",
                "a".repeat(n)
            );
            let mut body = raw_part(&disposition, b"{}");
            body.extend(closing());
            body
        };
        assert_eq!(
            ok_value(run(&content_type(), &long_name(4150), LIMIT)),
            json!({})
        );
        assert_400(run(&content_type(), &long_name(4300), LIMIT), E_FRAMING);
    }

    // ---- 重名 / request / 附件字段 --------------------------------------------------------------

    #[test]
    fn duplicate_field_names_are_rejected() {
        assert_400(
            parse(&[
                file("image", PNG),
                file("image", PNG),
                req(r#"{"image":"image"}"#),
            ]),
            E_DUPLICATE,
        );
        assert_400(parse(&[req(r#"{"a":1}"#), req(r#"{"b":2}"#)]), E_DUPLICATE);
        assert_400(
            parse(&[req_text(r#"{"a":1}"#), req(r#"{"b":2}"#)]),
            E_DUPLICATE,
        );
        assert_400(
            parse(&[field("extra", "1"), field("extra", "2"), req("{}")]),
            E_DUPLICATE,
        );
    }

    #[test]
    fn request_must_be_a_json_object() {
        for (what, payload) in [
            ("语法错误", "{not json"),
            ("空串", ""),
            ("NaN", r#"{"image": NaN}"#),
            ("Infinity", r#"{"x": [-Infinity]}"#),
            // 旧版放行、但 serde_json::Value 装不下的内容（孤立代理 / 超大整数 / 溢出浮点，即 `Lossy`）
            // 按「不是合法 JSON」拒绝，见模块文档
            ("孤立代理", r#"{"n":"\ud800"}"#),
            ("溢出浮点", r#"{"n":1e999}"#),
            ("超大整数", r#"{"n":123456789012345678901234567890}"#),
        ] {
            expect_err(what, parse(&[req(payload)]), 400, E_NOT_JSON);
        }
        for payload in ["[]", "5", "null", r#""image""#, "true"] {
            expect_err(payload, parse(&[req(payload)]), 400, E_NOT_OBJECT);
        }
    }

    #[test]
    fn only_known_file_parts_are_attachments() {
        for (what, name) in [
            ("未知名字", "extra"),
            ("编号不是数字", "ref_multiple_x"),
            ("编号为空", "ref_multiple_"),
            ("路径", "../private"),
            ("大小写", "Image"),
            ("空名字", ""),
            ("尾随空格", "image "),
            ("编号里夹字母", "director_ref_1a"),
        ] {
            expect_err(what, parse(&[file(name, PNG), req("{}")]), 400, E_BAD_FIELD);
        }
        // 图片字段名对，但是是普通字段（没有 filename）
        assert_400(
            parse(&[field("image", "text"), req(r#"{"image":"image"}"#)]),
            E_BAD_FIELD,
        );
        // 合法的名字
        for name in [
            "image",
            "mask",
            "reference_image",
            "ref_multiple_0",
            "ref_multiple_17",
            "director_ref_3",
        ] {
            let payload = json!({"parameters": {"image": name}}).to_string();
            let got = ok_value(parse(&[file(name, b"a"), req(payload)]));
            assert_eq!(got, json!({"parameters": {"image": "YQ=="}}), "{name}");
        }
    }

    #[test]
    fn content_disposition_spellings_follow_python_multipart() {
        // (头, 是否能当作名叫 image 的文件附件)
        let cases = [
            (r#"form-data; name="image"; filename="blob""#, true),
            (r#"form-data; name="image"; filename="""#, true), // 空 filename 也算文件
            (r#"form-data; name="image"; filename"#, true),
            (r#"form-data; name="image"; FILENAME="b""#, true),
            (r#"form-data; NAME="image"; filename="b""#, true),
            ("form-data; name=image; filename=b", true),
            (r#"form-data; filename="b"; name="image""#, true),
            (r#"form-data; name = "image" ; filename = "b""#, true),
            (r#"form-data; name="image"; filename="a;b""#, true),
            (r#"form-data; name="image"; filename="a\"; x="1"#, true),
            // `filename*=`（RFC 5987）被忽略，于是只是个普通字段
            (r#"form-data; name="image"; filename*=UTF-8''b.png"#, false),
            (r#"form-data; name="image""#, false),
            (r#"form-data; name="im\"age"; filename="b""#, false),
            (r#"form-data; name="ima;ge"; filename="b""#, false),
            (r#"form-data; name="image; filename="b""#, false),
        ];
        let request = raw_part(
            r#"form-data; name="request"; filename="blob""#,
            br#"{"image":"image"}"#,
        );
        for (disposition, is_file) in cases {
            let mut body = raw_part(disposition, PNG);
            body.extend(&request);
            body.extend(closing());
            let result = run(&content_type(), &body, LIMIT);
            if is_file {
                assert_eq!(
                    ok_value(result),
                    json!({"image": "iVBORw0KGgoAAQID"}),
                    "{disposition}"
                );
            } else {
                expect_err(disposition, result, 400, E_BAD_FIELD);
            }
        }
    }

    #[test]
    fn missing_referenced_attachment() {
        assert_400(parse(&[req(r#"{"image":"image"}"#)]), E_MISSING);
        assert_400(
            parse(&[file("image", PNG), req(r#"{"parameters":{"mask":"mask"}}"#)]),
            E_MISSING,
        );
        assert_400(
            parse(&[req(
                r#"{"parameters":{"reference_image_multiple_cached":[{"data":"ref_multiple_3"}]}}"#,
            )]),
            E_MISSING,
        );
        // 看起来像附件名的指针也要有对应附件；不像的当内联数据放行
        assert_400(parse(&[req(r#"{"image":"ref_multiple_12"}"#)]), E_MISSING);
        assert!(parse(&[req(r#"{"image":"ref_multiple_x"}"#)]).is_ok());
    }

    #[test]
    fn empty_attachment_is_rejected() {
        assert_400(
            parse(&[file("image", b""), req(r#"{"image":"image"}"#)]),
            E_EMPTY,
        );
        assert_400(
            parse(&[
                file("image", b""),
                req(r#"{"image":"image","mask":"image"}"#),
            ]),
            E_EMPTY,
        );
    }

    #[test]
    fn reference_lists_are_capped_at_sixteen() {
        let refs = |key: &str, n: usize| {
            json!({"parameters": {key: vec![json!({"data": "ref_multiple_0"}); n]}}).to_string()
        };
        let got = ok_value(parse(&[
            file("ref_multiple_0", b"a"),
            req(refs("reference_image_multiple_cached", 16)),
        ]));
        let list = got["parameters"]["reference_image_multiple_cached"]
            .as_array()
            .unwrap();
        assert_eq!(list.len(), 16);
        assert!(list.iter().all(|item| item == &json!({"data": "YQ=="})));
        assert_400(
            parse(&[
                file("ref_multiple_0", b"a"),
                req(refs("reference_image_multiple_cached", 17)),
            ]),
            E_REFS,
        );
        // 数量先于元素检查；顶层和 parameters 都查；两个列表字段都查
        let seventeen = vec![1; 17];
        assert_400(
            parse(&[req(
                json!({"parameters": {"director_reference_images_cached": seventeen}}).to_string(),
            )]),
            E_REFS,
        );
        assert_400(
            parse(&[req(
                json!({"director_reference_images_cached": seventeen}).to_string()
            )]),
            E_REFS,
        );
        // 只走顶层和 parameters 两层，更深的嵌套不碰
        let nested =
            json!({"parameters": {"nested": {"reference_image_multiple_cached": seventeen}}});
        assert_eq!(ok_value(parse(&[req(nested.to_string())])), nested);
        // 列表字段不是数组 / 元素不是对象：略过
        let odd = json!({"reference_image_multiple_cached": "image", "director_reference_images_cached": ["image", 3, [1]]});
        assert_eq!(ok_value(parse(&[req(odd.to_string())])), odd);
    }

    #[test]
    fn every_attachment_must_be_referenced() {
        assert_400(
            parse(&[
                file("image", PNG),
                file("mask", PNG),
                req(r#"{"image":"image"}"#),
            ]),
            E_UNUSED,
        );
        assert_400(
            parse(&[file("image", PNG), req(r#"{"input":"image"}"#)]),
            E_UNUSED,
        );
        // 指针藏在不会被展开的位置，附件仍算没被引用
        assert_400(
            parse(&[
                file("image", PNG),
                req(r#"{"input":"image","parameters":{"extra":{"image":"image"}}}"#),
            ]),
            E_UNUSED,
        );
    }

    // ---- 展开后大小（413）-----------------------------------------------------------------------

    #[test]
    fn expanded_size_limit_boundary() {
        // 体 = 一个 n 字节的 image 附件 + request `{"image":"image"}`，限额取整个体的长度：
        // 展开后大小 − 限额 在 n=913 / 916 / 919 时依次为 −1 / 0 / +1
        let parts = |n: usize| vec![file("image", &vec![b'x'; n]), req(r#"{"image":"image"}"#)];
        for n in [913, 916] {
            let body = build(&parts(n));
            let got = ok_value(run(&content_type(), &body, body.len()));
            assert_eq!(got["image"], json!(b64(&vec![b'x'; n])), "n={n}");
        }
        let body = build(&parts(919));
        assert_eq!(body.len(), 1239);
        expect_err(
            "n=919",
            run(&content_type(), &body, body.len()),
            413,
            E_EXPANDED,
        );
    }

    #[test]
    fn repeated_pointers_are_charged_every_time() {
        // 旧版测试 `test_repeated_pointer_expansion_cannot_bypass_json_size_limit` 的形态：
        // 同一个附件被两个槽位引用，每引用一次都计入展开后大小
        let body = build(&[
            file("image", &vec![b'x'; 3072]),
            req(r#"{"image":"image","mask":"image"}"#),
        ]);
        expect_err(
            "两次引用",
            run(&content_type(), &body, body.len()),
            413,
            E_EXPANDED,
        );
        // 300 字节 → 400 字节 Base64；限额 1000：2 次引用放行，3 次超限
        let uses = |n: usize| {
            let payload = json!({"parameters": {"director_reference_images_cached": vec![json!({"data": "director_ref_0"}); n]}});
            build(&[
                file("director_ref_0", &vec![b'x'; 300]),
                req(payload.to_string()),
            ])
        };
        assert!(run(&content_type(), &uses(2), 1000).is_ok());
        expect_err(
            "三次引用",
            run(&content_type(), &uses(3), 1000),
            413,
            E_EXPANDED,
        );
    }

    // ---- charset / Content-Disposition 解析 -----------------------------------------------------

    #[test]
    fn charset_param_selects_how_names_and_text_parts_are_decoded() {
        let body = |name: &[u8]| {
            let mut out =
                format!("--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"").into_bytes();
            out.extend_from_slice(name);
            out.extend_from_slice(
                format!("\"; filename=\"b\"\r\n\r\n{{}}\r\n--{BOUNDARY}--\r\n").as_bytes(),
            );
            out
        };
        // name 的字节不是 UTF-8 时按 latin-1 兜底，所以 `request\xe9` ≠ `request`；`é` 的 UTF-8 也不是 request
        assert_400(
            run(&content_type(), &body(b"request\xe9"), LIMIT),
            E_NO_REQUEST,
        );
        assert_400(
            run(&content_type(), &body("é".as_bytes()), LIMIT),
            E_NO_REQUEST,
        );
        assert_eq!(
            ok_value(run(&content_type(), &body(b"request"), LIMIT)),
            json!({})
        );
        // charset 参数的写法不影响 ASCII 名字
        for charset in ["latin-1", "UTF-8", "utf-16"] {
            let ct = format!("multipart/form-data; charset={charset}; boundary={BOUNDARY}");
            let body = build(&[file("image", PNG), req(r#"{"image":"image"}"#)]);
            assert_eq!(
                ok_value(run(&ct, &body, LIMIT))["image"],
                json!("iVBORw0KGgoAAQID")
            );
        }
        // 非 UTF-8 的字符集下，文本字段里的 UTF-8 多字节被拆成 latin-1 字符
        let text = build(&[req_text("{\"n\":\"é\"}")]);
        let ct = format!("multipart/form-data; charset=latin-1; boundary={BOUNDARY}");
        assert_eq!(ok_value(run(&ct, &text, LIMIT)), json!({"n": "Ã©"}));
        assert_eq!(
            ok_value(run(&content_type(), &text, LIMIT)),
            json!({"n": "é"})
        );
    }

    #[test]
    fn parse_options_header_matches_python_multipart() {
        fn expect(value: &str, media: &str, options: &[(&str, &str)]) {
            let (got_media, got) = parse_options_header(value.as_bytes());
            assert_eq!(got_media, media.as_bytes(), "{value:?}");
            let want: Options = options
                .iter()
                .map(|(k, v)| (k.as_bytes().to_vec(), v.as_bytes().to_vec()))
                .collect();
            assert_eq!(got, want, "{value:?}");
        }
        expect(
            r#"form-data; name="a"; filename="b.png""#,
            "form-data",
            &[("name", "a"), ("filename", "b.png")],
        );
        // `name*=` 一类扩展语法被忽略；带引号的分号不分段
        expect(
            r#"form-data; name="a;b"; filename*=UTF-8''x.png"#,
            "form-data",
            &[("name", "a;b")],
        );
        // 参数名转小写，主值不转（有参数时）
        expect(
            r#"Form-Data; NAME="x"; FileName=y"#,
            "Form-Data",
            &[("name", "x"), ("filename", "y")],
        );
        expect(
            r#"form-data; name="a\"b\\c""#,
            "form-data",
            &[("name", r#"a"b\c"#)],
        );
        // 没有 `=` 的参数值为空串，但键存在
        expect(
            "form-data; name=x; filename",
            "form-data",
            &[("name", "x"), ("filename", "")],
        );
        expect(
            r#"form-data; name="a"; name="b""#,
            "form-data",
            &[("name", "b")],
        );
        expect(
            r#"multipart/form-data; boundary="a b"; charset=UTF-8"#,
            "multipart/form-data",
            &[("boundary", "a b"), ("charset", "UTF-8")],
        );
        expect(
            "multipart/form-data;boundary=x;",
            "multipart/form-data",
            &[("boundary", "x"), ("", "")],
        );
        // 没有参数时才转小写
        expect("MULTIPART/FORM-DATA", "multipart/form-data", &[]);
        expect(
            "Multipart/Form-Data; boundary=x",
            "Multipart/Form-Data",
            &[("boundary", "x")],
        );
        // 引号没闭合：保留原样
        expect(
            r#"form-data; name="un;closed"#,
            "form-data",
            &[("name", r#""un;closed"#)],
        );
        expect(
            r#"form-data; name=""; filename="""#,
            "form-data",
            &[("name", ""), ("filename", "")],
        );
        expect("", "", &[]);
        expect("form-data;", "form-data", &[("", "")]);
        expect(
            r#"form-data; name="a" ; filename = "c""#,
            "form-data",
            &[("name", "a"), ("filename", "c")],
        );
        expect(
            r#"form-data; name="a;b"; filename="c;d"; x="1"#,
            "form-data",
            &[("name", "a;b"), ("filename", "c;d"), ("x", r#""1"#)],
        );
        expect("a=b; c=d", "a=b", &[("c", "d")]);
    }

    // ---- read_bounded_body ----------------------------------------------------------------------

    fn length_header(value: &[u8]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            CONTENT_LENGTH,
            axum::http::HeaderValue::from_bytes(value).unwrap(),
        );
        headers
    }

    /// 读完返回字节数，失败返回 (状态码, detail)
    async fn read(
        headers: &HeaderMap,
        chunks: &[&[u8]],
        limit: usize,
    ) -> Result<usize, (u16, String)> {
        let chunks: Vec<_> = chunks
            .iter()
            .map(|c| Ok::<_, std::io::Error>(Bytes::copy_from_slice(c)))
            .collect();
        let body = Body::from_stream(futures::stream::iter(chunks));
        read_bounded_body(headers, body, limit)
            .await
            .map(|bytes| bytes.len())
            .map_err(|e| (e.status, e.detail.as_str().unwrap().to_owned()))
    }

    fn too_large() -> (u16, String) {
        (413, "请求内容过大，未处理。请缩小图片或减少参考图。".into())
    }

    fn bad_length() -> (u16, String) {
        (
            400,
            "请求不完整，未处理。请重试或更换客户端。开发者参考：Content-Length 无效".into(),
        )
    }

    #[tokio::test]
    async fn content_length_header_checks_match_python() {
        let none = HeaderMap::new();
        let ten = b"x".repeat(10);
        assert_eq!(read(&none, &[b"abc"], 10).await, Ok(3));
        assert_eq!(read(&length_header(b"10"), &[&ten], 10).await, Ok(10));
        assert_eq!(
            read(&length_header(b"11"), &[&ten], 10).await,
            Err(too_large())
        );
        // 前导零不影响数值；纯零是 0
        assert_eq!(
            read(&length_header(b"0000000010"), &[&ten], 10).await,
            Ok(10)
        );
        assert_eq!(read(&length_header(b"000"), &[b""], 10).await, Ok(0));
        // 先比位数再比值：再长的数字串也只会得到 413
        let huge = "9".repeat(60);
        assert_eq!(
            read(&length_header(huge.as_bytes()), &[b"x"], 10).await,
            Err(too_large())
        );
        let padded = format!("{}11", "0".repeat(40));
        assert_eq!(
            read(&length_header(padded.as_bytes()), &[b"x"], 10).await,
            Err(too_large())
        );
        // 不是 ASCII 十进制数
        let bad: [&[u8]; 8] = [
            b"",
            b"12a",
            b"-1",
            b"+5",
            b" 5",
            b"0x5",
            "１２".as_bytes(),
            b"\xd9\xa1",
        ];
        for value in bad {
            assert_eq!(
                read(&length_header(value), &[b"x"], 10).await,
                Err(bad_length()),
                "{value:?}"
            );
        }
        // 限额为 0 / 1000 时的边界
        assert_eq!(read(&none, &[b""], 0).await, Ok(0));
        assert_eq!(read(&length_header(b"0"), &[b""], 0).await, Ok(0));
        assert_eq!(
            read(&length_header(b"1"), &[b""], 0).await,
            Err(too_large())
        );
        assert_eq!(read(&length_header(b"1000"), &[b"xxx"], 1000).await, Ok(3));
        assert_eq!(
            read(&length_header(b"1001"), &[b"xxx"], 1000).await,
            Err(too_large())
        );
    }

    #[tokio::test]
    async fn streamed_body_is_capped_even_if_the_header_lies() {
        // 声明 1 字节，实际发 11 字节：照样 413
        let eleven = b"x".repeat(11);
        assert_eq!(
            read(&length_header(b"1"), &[&eleven], 10).await,
            Err(too_large())
        );
        // 没有头，分块累计超限 / 恰好到限
        let none = HeaderMap::new();
        assert_eq!(
            read(&none, &[b"xxxxxx", b"yyyyy"], 10).await,
            Err(too_large())
        );
        assert_eq!(read(&none, &[b"xxxxxx", b"yyyy"], 10).await, Ok(10));
    }

    #[tokio::test]
    async fn client_disconnect_while_reading_is_a_400() {
        let none = HeaderMap::new();
        let mid_body: Vec<Result<Bytes, std::io::Error>> = vec![
            Ok(Bytes::from_static(b"abc")),
            Err(std::io::Error::new(
                std::io::ErrorKind::ConnectionReset,
                "reset",
            )),
        ];
        let before_body: Vec<Result<Bytes, std::io::Error>> = vec![Err(std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            "gone",
        ))];
        for chunks in [mid_body, before_body] {
            let body = Body::from_stream(futures::stream::iter(chunks));
            let e = read_bounded_body(&none, body, 100).await.unwrap_err();
            assert_eq!(
                (e.status, e.detail),
                (400, json!("上传中断，未处理。请检查网络后重试。"))
            );
        }
    }

    #[tokio::test]
    async fn bounded_body_feeds_the_multipart_parser() {
        let raw = build(&[file("image", PNG), req(r#"{"image":"image"}"#)]);
        let headers = length_header(raw.len().to_string().as_bytes());
        let body = read_bounded_body(&headers, Body::from(raw.clone()), LIMIT)
            .await
            .unwrap();
        assert_eq!(body.as_ref(), raw.as_slice());
        let got = parse_image_multipart(&content_type(), body, LIMIT).unwrap();
        assert_eq!(Value::Object(got), json!({"image": "iVBORw0KGgoAAQID"}));
    }
}
