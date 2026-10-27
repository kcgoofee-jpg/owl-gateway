//! 异常信号日志：把「不该发生」的错误按统一格式写一行结构化日志（`journalctl -u owl-gate | grep bug` 可查）。
//!
//! 来源：接口未捕获的错误（500）、上游 5xx、后台任务失败、记账写入失败、成员在图片返回前断开。
//! 不记录 Key、提示词或 IP；永远不会让调用方出错或阻塞。
//!
//! 10-27 起 Bug 追踪整个删除（`error_events` 表、后台「Bug 追踪」页、网页脚本报错上报、自动关闭任务）：
//! [`Bugs::capture`] 只写日志、不落库，12 个捕获点原样保留，信号在 journald 里仍然可查。

use gate_core::errors::should_print_stack;
use std::collections::HashMap;
use std::sync::Mutex;

pub struct Bugs {
    /// 同一特征的详细内容只完整打印一次（10 分钟内不重复刷屏）。
    printed: Mutex<HashMap<String, f64>>,
}

/// 一次记录的参数。
#[derive(Default, Clone)]
pub struct Report<'a> {
    pub source: &'a str,
    /// (错误类型名, 消息)；Rust 里没有 Python 的堆栈帧，位置用错误类型所在模块代替。
    pub error: Option<(&'a str, &'a str)>,
    pub title: &'a str,
    pub detail: &'a str,
    pub path: &'a str,
    pub key_id: Option<i64>,
    pub rid: &'a str,
    /// "error"（默认）或 "warn"
    pub level: &'a str,
}

impl Default for Bugs {
    fn default() -> Self {
        Self::new()
    }
}

impl Bugs {
    pub fn new() -> Self {
        Self {
            printed: Mutex::new(HashMap::new()),
        }
    }

    /// 兼容旧关停序列：以前等异步入库排空，现在日志是同步写的，无事可做。
    pub async fn idle(&self) {}

    /// 写一行结构化日志；返回特征码（旧版入库归并用，现在没有调用方消费返回值）。
    pub fn capture(&self, r: Report<'_>) -> String {
        let now = crate::time::now();
        let (ty, msg) = r.error.unwrap_or(("Error", r.title));
        let sig = format!("{ty}:{msg}:{}/{ty}", r.path);
        let level = if r.level.is_empty() { "error" } else { r.level };
        let print_stack = r.error.is_some()
            && should_print_stack(now, self.printed.lock().expect("bugs").get(&sig).copied());
        self.printed.lock().expect("bugs").insert(sig.clone(), now);
        let title = if r.title.is_empty() { msg } else { r.title };
        let tail = if print_stack && !r.detail.is_empty() {
            format!("\n{}", r.detail)
        } else {
            String::new()
        };
        let record = |level| {
            tracing::warn!(
                target: "bug",
                source = %r.source,
                path = %r.path,
                rid = %r.rid,
                key_id = r.key_id,
                stack = print_stack,
                level,
                "[bug] {title}: {ty}: {msg}{tail}"
            );
        };
        match level {
            "warn" => record("warn"),
            _ => record("error"),
        }
        sig
    }
}
