//! 真实二进制的优雅关停：SIGTERM 到达时即使浏览器还开着实时推送（SSE），进程也要很快退出，
//! 并且按顺序收尾——取消后台循环 → HTTP 停止接收 → 等记账任务 → 状态码统计落库 → WAL 并回主库。
//!
//! 只在 unix 上跑（用 `kill -TERM`）。公开仓库：全部是合成数据，不连任何外部服务。

#![cfg(unix)]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn spawn_gateway(port: u16, data: &std::path::Path) -> Child {
    Command::new(env!("CARGO_BIN_EXE_nai-gate"))
        .args(["serve", "--bind", &format!("127.0.0.1:{port}")])
        .env_clear()
        .env("DATA_DIR", data)
        .env("NAI_TOKENS", "synthetic-upstream-token")
        .env("NAI_IMAGE_HOST", "http://127.0.0.1:9")
        .env("ADMIN_PASSWORD", "dev-admin-password-123")
        .env("SECRET_KEY", "test-secret-key-test-secret-key-0123456789")
        // 维护循环启动就会取一次官方状态：指到不可达的本机端口，保证不出本机
        .env("OFFICIAL_STATUS_URL", "http://127.0.0.1:9/never")
        .env("RUST_LOG", "info")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("启动网关二进制")
}

fn get(port: u16, path: &str, extra: &str) -> TcpStream {
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("连接网关");
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    write!(s, "GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n{extra}\r\n").unwrap();
    s
}

fn wait_healthy(port: u16, child: &mut Child) {
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            panic!("网关提前退出：{status}");
        }
        if let Ok(mut s) = TcpStream::connect(("127.0.0.1", port)) {
            s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            let _ = write!(
                s,
                "GET /healthz HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n"
            );
            let mut buf = String::new();
            let _ = s.read_to_string(&mut buf);
            if buf.starts_with("HTTP/1.1 200") {
                return;
            }
        }
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "网关 30 秒内没起来"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn sigterm_with_open_live_stream_exits_promptly_and_cleanly() {
    let data = tempfile::tempdir().unwrap();
    let port = free_port();
    let mut child = spawn_gateway(port, data.path());
    wait_healthy(port, &mut child);

    // 打开实时推送：读到响应头 + 第一个事件，连接保持
    let mut sse = get(port, "/public/live/stream", "Accept: text/event-stream\r\n");
    let mut first = [0u8; 512];
    let n = sse.read(&mut first).expect("读 SSE 响应头");
    let head = String::from_utf8_lossy(&first[..n]).to_string();
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert!(head.to_lowercase().contains("text/event-stream"), "{head}");

    // 再来一个普通请求（计入状态码统计），确认服务正常
    let mut other = get(port, "/healthz", "Connection: close\r\n");
    let mut buf = String::new();
    other.read_to_string(&mut buf).unwrap();
    assert!(buf.starts_with("HTTP/1.1 200"));

    let t0 = Instant::now();
    let status = Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(status.success());
    let exit = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break s;
        }
        if t0.elapsed() > Duration::from_secs(15) {
            let _ = child.kill();
            panic!("SIGTERM 之后 15 秒还没退出：开着 SSE 时优雅关停卡住了");
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    let took = t0.elapsed();
    assert!(exit.success(), "退出码不是 0：{exit}");
    assert!(took < Duration::from_secs(10), "关停太慢：{took:?}");

    // SSE 连接被服务端正常结束（读到 EOF，而不是超时）
    let mut rest = Vec::new();
    match sse.read_to_end(&mut rest) {
        Ok(_) => {}
        Err(e) => assert!(
            !matches!(
                e.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
            ),
            "SSE 连接没有被关闭：{e}"
        ),
    }
    // WAL 已并回主库（TRUNCATE）：-wal 文件不存在或为空
    let wal = data.path().join("nai_gate.db-wal");
    let wal_len = std::fs::metadata(&wal).map(|m| m.len()).unwrap_or(0);
    assert_eq!(wal_len, 0, "关停后 WAL 应该已经并回主库");
}
