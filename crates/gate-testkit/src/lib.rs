//! 可编程的假 NovelAI 上游。
//!
//! - 默认行为（不写脚本时）：出图返回装着 `n_samples` 张真 PNG 的 zip；流式出图按 `parameters.stream`
//!   返回 msgpack（4 字节大端长度 + map）或 SSE，先若干 intermediate 再 final；
//!   订阅查询返回 Opus + V5 剩余 80%；补全返回固定标签；放大 / 导演工具返回 PNG；Vibe 编码返回二进制。
//! - 脚本：`push(path, Reply)` 让该路径的下一次请求按脚本回（429 带 Retry-After、403 模型不存在、5xx、
//!   流到一半断开、永不结束的流、卡住不回……），用完回到默认行为。
//! - 记录：每个请求的方法、路径、查询串、请求头（按收到的顺序）、正文都记下来，供对拍上游请求日志。
//! - 压缩：`set_encoding(Some("br"))` 之后，请求的 Accept-Encoding 里有这一种就把响应按它压缩
//!   （gzip / deflate / br / zstd），每个分块压完立刻 flush——流式回复照样一块一块到，可以测「边收边转」。

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::Response;
use base64::Engine;
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// 记录下来的一个上游请求。
#[derive(Debug, Clone)]
pub struct Recorded {
    pub method: String,
    pub path: String,
    pub query: String,
    /// (小写名, 值)，按收到的顺序。
    pub headers: Vec<(String, String)>,
    pub body: Bytes,
}

impl Recorded {
    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}

/// 脚本化的回复。
#[derive(Debug, Clone)]
pub enum Reply {
    /// 状态码 + 头 + 正文。
    Status {
        status: u16,
        headers: Vec<(String, String)>,
        body: Bytes,
    },
    /// 分块回复（每块之前等一会儿），可模拟中途断开（最后一块后直接结束连接）。
    Chunks {
        status: u16,
        content_type: String,
        chunks: Vec<(Duration, Bytes)>,
    },
    /// 永不返回（测试取消 / 排队超时）。
    Hang,
}

impl Reply {
    pub fn status(status: u16, body: impl Into<Bytes>) -> Self {
        Reply::Status {
            status,
            headers: vec![],
            body: body.into(),
        }
    }
    pub fn json(status: u16, v: &Value) -> Self {
        Reply::Status {
            status,
            headers: vec![("content-type".into(), "application/json".into())],
            body: Bytes::from(serde_json::to_vec(v).unwrap_or_default()),
        }
    }
    /// 上游 429，带 Retry-After。
    pub fn rate_limited(retry_after: &str) -> Self {
        Reply::Status {
            status: 429,
            headers: vec![("retry-after".into(), retry_after.into())],
            body: Bytes::from_static(br#"{"statusCode":429,"message":"Too many requests"}"#),
        }
    }
    /// 上游 403「模型不存在」（2026-10-13 实测原文）。
    pub fn model_403(model: &str) -> Self {
        Reply::json(
            403,
            &json!({"statusCode": 403, "message": format!("model '{model}' doesn't exist")}),
        )
    }
    /// 上游「并发锁」：同一个账号的上一张还没释放（NovelAI 原文；状态码一般是 429，偶见 409）。
    /// 不是限流：网关不冷却，稍后在同一账号上重试。
    pub fn concurrency_locked(status: u16) -> Self {
        Reply::json(
            status,
            &json!({"statusCode": status, "message": "Concurrent generation is locked"}),
        )
    }
    /// 上游 403 账号级拒绝。
    pub fn account_403() -> Self {
        Reply::json(403, &json!({"statusCode": 403, "message": "Forbidden"}))
    }
}

#[derive(Default)]
struct MockState {
    requests: Mutex<Vec<Recorded>>,
    scripts: Mutex<HashMap<String, VecDeque<Reply>>>,
    /// 订阅查询回的 V5 剩余百分比（默认 80）。
    v5_percent: Mutex<i64>,
    /// 响应压缩（None = 不压缩）。
    encoding: Mutex<Option<&'static str>>,
    /// 实际压缩过的响应：(路径, 编码)。
    encoded: Mutex<Vec<(String, String)>>,
}

/// 运行中的假上游。Drop 时服务停止。
pub struct MockUpstream {
    pub addr: SocketAddr,
    state: Arc<MockState>,
    _shutdown: tokio::sync::oneshot::Sender<()>,
}

impl MockUpstream {
    /// 绑定本机随机端口启动（测试用）。
    pub async fn start() -> Self {
        Self::start_on("127.0.0.1:0")
            .await
            .expect("绑定本机随机端口")
    }

    /// 绑定指定地址启动（开发用：`cargo xtask dev` 需要固定端口）。端口被占用时返回错误而不是 panic。
    pub async fn start_on(bind: impl tokio::net::ToSocketAddrs) -> std::io::Result<Self> {
        let state = Arc::new(MockState {
            v5_percent: Mutex::new(80),
            ..Default::default()
        });
        let app = Router::new().fallback(handle).with_state(state.clone());
        let listener = tokio::net::TcpListener::bind(bind).await?;
        let addr = listener.local_addr()?;
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = rx.await;
                })
                .await;
        });
        Ok(Self {
            addr,
            state,
            _shutdown: tx,
        })
    }

    /// `http://127.0.0.1:<port>`
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// 让 `path` 的下一次请求按脚本回复（可多次 push，按顺序消费）。
    pub fn push(&self, path: &str, reply: Reply) {
        self.state
            .scripts
            .lock()
            .expect("scripts")
            .entry(path.to_string())
            .or_default()
            .push_back(reply);
    }

    /// 之后的响应按 `encoding`（gzip / deflate / br / zstd）压缩——前提是请求的 Accept-Encoding 里有它。
    pub fn set_encoding(&self, encoding: Option<&str>) {
        let e = encoding.map(|e| match e {
            "gzip" => "gzip",
            "deflate" => "deflate",
            "br" => "br",
            "zstd" => "zstd",
            other => panic!("假上游不支持的压缩：{other}"),
        });
        *self.state.encoding.lock().expect("encoding") = e;
    }

    /// 实际压缩过的响应：(路径, 编码)，按顺序。
    pub fn encoded_responses(&self) -> Vec<(String, String)> {
        self.state.encoded.lock().expect("encoded").clone()
    }

    pub fn set_v5_percent(&self, pct: i64) {
        *self.state.v5_percent.lock().expect("v5") = pct;
    }

    /// 至今收到的全部请求。
    pub fn requests(&self) -> Vec<Recorded> {
        self.state.requests.lock().expect("requests").clone()
    }

    /// 只看某个路径的请求。
    pub fn requests_to(&self, path: &str) -> Vec<Recorded> {
        self.requests()
            .into_iter()
            .filter(|r| r.path == path)
            .collect()
    }

    pub fn clear(&self) {
        self.state.requests.lock().expect("requests").clear();
    }
}

async fn handle(State(state): State<Arc<MockState>>, req: Request) -> Response {
    let method = req.method().to_string();
    let path = req.uri().path().to_string();
    let query = req.uri().query().unwrap_or("").to_string();
    let headers: Vec<(String, String)> = req
        .headers()
        .iter()
        .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
        .collect();
    let accepted = req
        .headers()
        .get("accept-encoding")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let body = axum::body::to_bytes(req.into_body(), 256 * 1024 * 1024)
        .await
        .unwrap_or_default();
    state.requests.lock().expect("requests").push(Recorded {
        method: method.clone(),
        path: path.clone(),
        query: query.clone(),
        headers,
        body: body.clone(),
    });
    let scripted = state
        .scripts
        .lock()
        .expect("scripts")
        .get_mut(&path)
        .and_then(VecDeque::pop_front);
    let resp = match scripted {
        Some(reply) => render(reply).await,
        None => default_reply(&state, &method, &path, &query, &body).await,
    };
    let encoding = *state.encoding.lock().expect("encoding");
    match encoding {
        Some(enc) if accepts(&accepted, enc) => {
            state
                .encoded
                .lock()
                .expect("encoded")
                .push((path, enc.to_string()));
            compress(resp, enc)
        }
        _ => resp,
    }
}

/// `Accept-Encoding` 里有没有这种编码（忽略 q 值与空白）。
fn accepts(header: &str, encoding: &str) -> bool {
    header
        .split(',')
        .any(|item| item.split(';').next().unwrap_or("").trim() == encoding)
}

/// 增量压缩器：每块写完就 flush，压出来的字节立刻能被解开。
enum Coder {
    Gzip(flate2::write::GzEncoder<Vec<u8>>),
    Deflate(flate2::write::ZlibEncoder<Vec<u8>>),
    Br(Box<brotli::CompressorWriter<Vec<u8>>>),
    Zstd(zstd::stream::write::Encoder<'static, Vec<u8>>),
}

impl Coder {
    fn new(encoding: &str) -> Self {
        let level = flate2::Compression::default();
        match encoding {
            "gzip" => Coder::Gzip(flate2::write::GzEncoder::new(Vec::new(), level)),
            "deflate" => Coder::Deflate(flate2::write::ZlibEncoder::new(Vec::new(), level)),
            "br" => Coder::Br(Box::new(brotli::CompressorWriter::new(
                Vec::new(),
                4096,
                5,
                22,
            ))),
            _ => Coder::Zstd(zstd::stream::write::Encoder::new(Vec::new(), 3).expect("zstd")),
        }
    }

    fn push(&mut self, data: &[u8]) -> Vec<u8> {
        fn write_flush(w: &mut impl Write, data: &[u8]) {
            w.write_all(data).expect("压缩");
            w.flush().expect("压缩");
        }
        match self {
            Coder::Gzip(e) => {
                write_flush(e, data);
                std::mem::take(e.get_mut())
            }
            Coder::Deflate(e) => {
                write_flush(e, data);
                std::mem::take(e.get_mut())
            }
            Coder::Br(e) => {
                write_flush(e.as_mut(), data);
                std::mem::take(e.get_mut())
            }
            Coder::Zstd(e) => {
                write_flush(e, data);
                std::mem::take(e.get_mut())
            }
        }
    }

    fn finish(self) -> Vec<u8> {
        match self {
            Coder::Gzip(e) => e.finish().expect("压缩"),
            Coder::Deflate(e) => e.finish().expect("压缩"),
            Coder::Br(e) => e.into_inner(),
            Coder::Zstd(e) => e.finish().expect("压缩"),
        }
    }
}

/// 把响应体按 `encoding` 逐块压缩（带 Content-Encoding，去掉 Content-Length）。
fn compress(resp: Response, encoding: &'static str) -> Response {
    use futures::StreamExt;
    let (mut parts, body) = resp.into_parts();
    parts
        .headers
        .insert("content-encoding", HeaderValue::from_static(encoding));
    parts.headers.remove("content-length");
    let coder = Arc::new(Mutex::new(Some(Coder::new(encoding))));
    let tail = coder.clone();
    let chunks = body.into_data_stream().map(move |chunk| {
        chunk.map(|data| {
            let mut c = coder.lock().expect("coder");
            Bytes::from(c.as_mut().expect("未结束").push(&data))
        })
    });
    let end = futures::stream::once(async move {
        let c = tail.lock().expect("coder").take().expect("只结束一次");
        Ok(Bytes::from(c.finish()))
    });
    Response::from_parts(parts, Body::from_stream(chunks.chain(end)))
}

async fn render(reply: Reply) -> Response {
    match reply {
        Reply::Status {
            status,
            headers,
            body,
        } => {
            let mut resp = Response::new(Body::from(body));
            *resp.status_mut() =
                StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            for (k, v) in headers {
                if let (Ok(name), Ok(val)) = (
                    k.parse::<axum::http::HeaderName>(),
                    HeaderValue::from_str(&v),
                ) {
                    resp.headers_mut().insert(name, val);
                }
            }
            resp
        }
        Reply::Chunks {
            status,
            content_type,
            chunks,
        } => {
            let stream = futures::stream::unfold(chunks.into_iter(), |mut it| async move {
                let (delay, data) = it.next()?;
                if !delay.is_zero() {
                    tokio::time::sleep(delay).await;
                }
                Some((Ok::<Bytes, std::io::Error>(data), it))
            });
            let mut resp = Response::new(Body::from_stream(stream));
            *resp.status_mut() = StatusCode::from_u16(status).unwrap_or(StatusCode::OK);
            if let Ok(v) = HeaderValue::from_str(&content_type) {
                resp.headers_mut().insert("content-type", v);
            }
            resp
        }
        Reply::Hang => {
            futures::future::pending::<()>().await;
            unreachable!()
        }
    }
}

/// 一张最小的真 PNG（64×64，能被图片库解码、能过「完整图片」校验）。
pub fn tiny_png() -> Vec<u8> {
    let img = image::RgbImage::from_fn(64, 64, |x, y| {
        image::Rgb([(x * 4) as u8, (y * 4) as u8, 128])
    });
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(img)
        .write_to(&mut out, image::ImageFormat::Png)
        .expect("编码 PNG");
    out.into_inner()
}

/// 与 NovelAI 一样的 zip：image_0.png …
pub fn zip_of_pngs(n: usize) -> Vec<u8> {
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut z = zip::ZipWriter::new(&mut buf);
        let png = tiny_png();
        for i in 0..n {
            z.start_file(
                format!("image_{i}.png"),
                zip::write::SimpleFileOptions::default(),
            )
            .expect("zip");
            z.write_all(&png).expect("zip");
        }
        z.finish().expect("zip");
    }
    buf.into_inner()
}

/// msgpack 帧：4 字节大端长度 + map。
pub fn msgpack_frame(pairs: Vec<(&str, rmpv::Value)>) -> Vec<u8> {
    let map = rmpv::Value::Map(
        pairs
            .into_iter()
            .map(|(k, v)| (rmpv::Value::from(k), v))
            .collect(),
    );
    let mut data = Vec::new();
    rmpv::encode::write_value(&mut data, &map).expect("msgpack");
    let mut out = (data.len() as u32).to_be_bytes().to_vec();
    out.extend(data);
    out
}

/// 一条 SSE 事件。
pub fn sse_event(event: &str, data: &Value) -> Vec<u8> {
    format!(
        "event: {event}\ndata: {}\n\n",
        serde_json::to_string(data).unwrap_or_default()
    )
    .into_bytes()
}

/// 一次完整的流式出图：每张 2 个 intermediate + 1 个 final。
pub fn image_stream(n: usize, wire: &str) -> Vec<(Duration, Bytes)> {
    let png = tiny_png();
    let b64 = base64::engine::general_purpose::STANDARD.encode(&png);
    let mut chunks = Vec::new();
    for i in 0..n {
        for step in 0..2 {
            let data = if wire == "msgpack" {
                msgpack_frame(vec![
                    ("event_type", "intermediate".into()),
                    ("samp_ix", (i as i64).into()),
                    ("step_ix", (step as i64).into()),
                    ("image", rmpv::Value::Binary(png.clone())),
                ])
            } else {
                sse_event(
                    "intermediate",
                    &json!({"event_type": "intermediate", "samp_ix": i, "step_ix": step, "image": b64}),
                )
            };
            chunks.push((Duration::from_millis(5), Bytes::from(data)));
        }
        let data = if wire == "msgpack" {
            msgpack_frame(vec![
                ("event_type", "final".into()),
                ("samp_ix", (i as i64).into()),
                ("image", rmpv::Value::Binary(png.clone())),
            ])
        } else {
            sse_event(
                "final",
                &json!({"event_type": "final", "samp_ix": i, "image": b64}),
            )
        };
        chunks.push((Duration::from_millis(5), Bytes::from(data)));
    }
    chunks
}

async fn default_reply(
    state: &MockState,
    method: &str,
    path: &str,
    query: &str,
    body: &Bytes,
) -> Response {
    let json_body: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    let n = json_body
        .get("parameters")
        .and_then(|p| p.get("n_samples"))
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .max(1) as usize;
    match (method, path) {
        ("POST", "/ai/generate-image") => {
            let mut h = HeaderMap::new();
            h.insert(
                "content-type",
                HeaderValue::from_static("binary/octet-stream"),
            );
            bytes_response(200, h, zip_of_pngs(n))
        }
        ("POST", "/ai/generate-image-stream") => {
            let wire = json_body
                .get("parameters")
                .and_then(|p| p.get("stream"))
                .and_then(Value::as_str)
                .unwrap_or("sse")
                .to_string();
            let ct = if wire == "msgpack" {
                "application/x-msgpack"
            } else {
                "text/event-stream"
            };
            render(Reply::Chunks {
                status: 200,
                content_type: ct.into(),
                chunks: image_stream(n, &wire),
            })
            .await
        }
        ("GET", "/user/subscription") => {
            let pct = *state.v5_percent.lock().expect("v5");
            json_response(
                200,
                &json!({
                    "tier": 3, "active": true, "expiresAt": 1893456000,
                    "perks": {"maxPriorityActions": 1000, "startPriority": 10, "unlimitedMaxPriority": true},
                    "trainingStepsLeft": {"fixedTrainingStepsLeft": 10000, "purchasedTrainingSteps": 0},
                    "usage": {"percent": pct, "isNegative": pct <= 0, "timeUntilNextPercent": 3600}
                }),
            )
        }
        ("GET", "/ai/generate-image/suggest-tags") => {
            let _ = query;
            json_response(
                200,
                &json!({"tags": [{"tag": "1girl", "count": 1000, "confidence": 0.9}]}),
            )
        }
        ("POST", "/ai/upscale") | ("POST", "/ai/augment-image") => {
            let mut h = HeaderMap::new();
            h.insert(
                "content-type",
                HeaderValue::from_static("binary/octet-stream"),
            );
            bytes_response(200, h, zip_of_pngs(1))
        }
        ("POST", "/ai/encode-vibe") => {
            let mut h = HeaderMap::new();
            h.insert(
                "content-type",
                HeaderValue::from_static("application/octet-stream"),
            );
            bytes_response(200, h, vec![7u8; 128])
        }
        _ => json_response(404, &json!({"statusCode": 404, "message": "Not Found"})),
    }
}

fn json_response(status: u16, v: &Value) -> Response {
    let mut h = HeaderMap::new();
    h.insert("content-type", HeaderValue::from_static("application/json"));
    bytes_response(status, h, serde_json::to_vec(v).unwrap_or_default())
}

fn bytes_response(status: u16, headers: HeaderMap, body: Vec<u8>) -> Response {
    let mut resp = Response::new(Body::from(body));
    *resp.status_mut() = StatusCode::from_u16(status).unwrap_or(StatusCode::OK);
    *resp.headers_mut() = headers;
    resp
}
