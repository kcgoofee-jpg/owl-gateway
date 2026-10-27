//! 「不该发生的错误」的日志限频：同一个 bug 的完整堆栈 10 分钟内只打印一次。
//! 10-27 起 Bug 追踪（error_events 表、后台页、网页上报）整个删除，出问题看日志（`target: "bug"`）。

/// 同一个 bug 的完整堆栈多久只打印一次（秒）。
pub const PRINT_STACK_INTERVAL: f64 = 600.0;

/// 同一个 bug 的堆栈 10 分钟内只打印一次：`now - last.get(sig, 0) > 600`。
pub fn should_print_stack(now: f64, last_printed: Option<f64>) -> bool {
    now - last_printed.unwrap_or(0.0) > PRINT_STACK_INTERVAL
}
