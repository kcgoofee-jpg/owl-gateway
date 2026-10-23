//! Bug 追踪的快照：特征码、事件构建、日志行、堆栈打印节流。
//! 特征码一变，线上已有的 Bug 就去不了重——`signature` 的快照就是兜底，另外下面手写了几个固定输入。

mod common;

use std::collections::HashMap;

use common::Tally;
use gate_core::errors::{
    self, Capture, ErrorEvent, ExcInfo, ExcKey, build_event, normalize_title, should_print_stack,
};
use gate_snapshot::snapshot::check;
use serde_json::{Value, json};

const M: &str = env!("CARGO_MANIFEST_DIR");

#[test]
fn constants() {
    assert_eq!(errors::LEVELS, ["error", "warn"]);
    assert_eq!(errors::DETAIL_MAX, 4000);
}

/// 特征码的字面值：这几个固定输入不能变。
#[test]
fn signature_literals() {
    assert_eq!(errors::signature("background", None, ""), "65eb0449ca04");
    assert_eq!(
        errors::signature(
            "frontend",
            Some(ExcKey {
                type_name: "ValueError",
                app_frame: "z.py:prefix_quirk_0"
            }),
            "自定义标题 502"
        ),
        "db13e5fe9f5c"
    );
    assert_eq!(
        errors::signature(
            "upstream_5xx",
            Some(ExcKey {
                type_name: "TimeoutError_",
                app_frame: "nai.py:request_0"
            }),
            "999999999999999999999999999999"
        ),
        "a32abb379075"
    );
    // 没有异常时看折叠数字后的标题：502 / 503 归到同一条
    assert_eq!(
        errors::signature("upstream", None, "上游返回 502"),
        errors::signature("upstream", None, "上游返回 503")
    );
    assert_eq!(normalize_title("耗时 1.5s / 2.5s"), "耗时 #s / #s");
}

#[test]
fn signature_snapshot() {
    let mut tally = Tally::default();
    check(M, "errors/signature", |c| {
        Some(match c.get("fn")?.as_str()? {
            "normalize_title" => {
                tally.add("normalize_title");
                json!(normalize_title(c.get("title")?.as_str()?))
            }
            "signature" => {
                let source = c.get("source")?.as_str()?;
                let title = c.get("title")?.as_str()?;
                let exc = c.get("exc")?;
                if exc.is_null() {
                    tally.add("without_exc");
                    json!(errors::signature(source, None, title))
                } else {
                    tally.add("with_exc");
                    let key = ExcKey {
                        type_name: exc["type"].as_str()?,
                        app_frame: exc["app_frame"].as_str()?,
                    };
                    json!(errors::signature(source, Some(key), title))
                }
            }
            other => panic!("未知函数 {other}"),
        })
    });
    tally.assert_at_least(&[
        ("normalize_title", 20),
        ("with_exc", 20),
        ("without_exc", 20),
    ]);
}

fn event_json(e: &ErrorEvent) -> Value {
    json!({
        "sig": e.sig, "source": e.source, "level": e.level, "title": e.title, "detail": e.detail,
        "path": e.path, "key_id": e.key_id, "rid": e.rid, "ts": gate_snapshot::f(e.ts),
    })
}

/// 一串 capture：每步记事件、日志行、这一步有没有打印堆栈（带异常且距上次打印超过 600 秒才打印）。
#[test]
fn capture_snapshot() {
    let mut tally = Tally::default();
    check(M, "errors/capture", |c| {
        let mut printed: HashMap<String, f64> = HashMap::new();
        let outs: Vec<Value> = c
            .get("steps")?
            .as_array()?
            .iter()
            .map(|step| {
                let s = |k: &str| step[k].as_str().unwrap();
                let exc = &step["exc"];
                let info = (!exc.is_null()).then(|| ExcInfo {
                    type_name: exc["type"].as_str().unwrap(),
                    message: exc["message"].as_str().unwrap(),
                    app_frame: exc["app_frame"].as_str().unwrap(),
                    stack: exc["stack"].as_str().unwrap(),
                });
                let input = Capture {
                    source: s("source"),
                    exc: info,
                    title: s("title"),
                    detail: s("detail"),
                    path: s("path"),
                    key_id: step["key_id"].as_i64(),
                    rid: s("rid"),
                    level: s("level"),
                };
                let now = step["now"].as_f64().unwrap();
                let got = build_event(&input, now);
                let print = input.exc.is_some()
                    && should_print_stack(now, printed.get(&got.event.sig).copied());
                if print {
                    printed.insert(got.event.sig.clone(), now);
                }
                tally.add_if(print, "stack_printed");
                tally.add_if(input.exc.is_some() && !print, "stack_throttled");
                tally.add_if(
                    got.event.detail.chars().count() == errors::DETAIL_MAX,
                    "detail_truncated",
                );
                json!({
                    "sig": got.event.sig,
                    "event": event_json(&got.event),
                    "log_line": got.log_line,
                    "stack_printed": print,
                    "stack_text": print.then_some(got.stack_text.as_str()),
                })
            })
            .collect();
        tally.add("trace");
        Some(Value::Array(outs))
    });
    tally.assert_at_least(&[
        ("trace", 5),
        ("stack_printed", 10),
        ("stack_throttled", 3),
        ("detail_truncated", 1),
    ]);
}
