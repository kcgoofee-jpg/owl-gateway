//! 异常信号日志：把「不该发生」的错误按统一格式写一行结构化日志（`journalctl -u owl-gate | grep bug` 可查）。
//!
//! 来源：接口未捕获的错误（500）、上游 5xx、后台任务失败和记账写入失败。
//! 不记录 Key 凭据、提示词或 IP；同一异常的详细内容每 10 分钟最多输出一次。
//!

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

    /// 写一行结构化日志，定期输出详细信息。
    pub fn capture(&self, r: Report<'_>) {
        let now = crate::time::now();
        let (ty, msg) = r.error.unwrap_or(("Error", r.title));
        let sig = format!("{ty}:{msg}:{}/{ty}", r.path);
        let level = if r.level.is_empty() { "error" } else { r.level };
        let print_stack = if r.error.is_some() {
            let mut printed = self.printed.lock().expect("bugs");
            printed.retain(|_, last| now - *last <= 600.0);
            let print = should_print_stack(now, printed.get(&sig).copied());
            if print {
                if printed.len() >= 1024
                    && let Some(oldest) = printed
                        .iter()
                        .min_by(|a, b| a.1.total_cmp(b.1))
                        .map(|(k, _)| k.clone())
                {
                    printed.remove(&oldest);
                }
                printed.insert(sig, now);
            }
            print
        } else {
            false
        };
        let title = if r.title.is_empty() { msg } else { r.title };
        let tail = if print_stack && !r.detail.is_empty() {
            format!("\n{}", r.detail)
        } else {
            String::new()
        };
        if level == "warn" {
            tracing::warn!(
                target: "bug",
                source = %r.source,
                path = %r.path,
                rid = %r.rid,
                key_id = r.key_id,
                stack = print_stack,
                "[bug] {title}: {ty}: {msg}{tail}"
            );
        } else {
            tracing::error!(
                target: "bug",
                source = %r.source,
                path = %r.path,
                rid = %r.rid,
                key_id = r.key_id,
                stack = print_stack,
                "[bug] {title}: {ty}: {msg}{tail}"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requested_levels_reach_the_logging_subscriber() {
        #[derive(Clone)]
        struct Buffer(std::sync::Arc<Mutex<Vec<u8>>>);
        impl std::io::Write for Buffer {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let buffer = Buffer(Default::default());
        let writer = buffer.clone();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish();
        tracing::subscriber::with_default(subscriber, || {
            let bugs = Bugs::new();
            bugs.capture(Report {
                title: "error-marker",
                ..Default::default()
            });
            bugs.capture(Report {
                title: "warn-marker",
                level: "warn",
                ..Default::default()
            });
        });
        let output = String::from_utf8(buffer.0.lock().unwrap().clone()).unwrap();
        assert!(
            output
                .lines()
                .any(|l| l.contains("ERROR") && l.contains("error-marker")),
            "{output}"
        );
        assert!(
            output
                .lines()
                .any(|l| l.contains("WARN") && l.contains("warn-marker")),
            "{output}"
        );
    }

    #[test]
    fn repeated_errors_do_not_postpone_detailed_logging() {
        let bugs = Bugs::new();
        let report = Report {
            error: Some(("Example", "failure")),
            ..Default::default()
        };
        bugs.capture(report.clone());
        let first = *bugs.printed.lock().unwrap().values().next().unwrap();
        bugs.capture(report);
        assert_eq!(
            *bugs.printed.lock().unwrap().values().next().unwrap(),
            first
        );
    }

    #[test]
    fn retained_signatures_are_bounded_and_expire() {
        let bugs = Bugs::new();
        let now = crate::time::now();
        {
            let mut printed = bugs.printed.lock().unwrap();
            printed.insert("expired".into(), now - 601.0);
            for i in 0..1024 {
                printed.insert(i.to_string(), now - 1.0);
            }
        }
        bugs.capture(Report {
            error: Some(("Example", "new")),
            ..Default::default()
        });
        let printed = bugs.printed.lock().unwrap();
        assert_eq!(printed.len(), 1024);
        assert!(!printed.contains_key("expired"));
    }
}
