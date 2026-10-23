//! Bug 追踪的纯逻辑（`app/errors.py`）：错误特征码、事件字段的截断规则。3.0 不再私信「新 bug / 复发」（非重大事故只写日志）。
//!
//! 不移植的部分：
//! - `Tracker` 的 I/O（`_store_locked` 的 SQL、`list` / `resolve` / `purge` 等）。表是 `error_events`
//!   （`sig` 为键，`count` 累加，`resolved_at` 非空表示站长标记过「已处理」）。
//! - `_app_frame(exc)`：从 Python 回溯里找「最里层属于本项目代码的那一帧」，格式 `文件名.py:函数名`，
//!   用来区分「同一种异常、不同位置」。Rust 没有 Python 回溯，调用方自己决定「位置」字符串怎么取
//!   （例如 panic 的 `file:line` 或模块路径），传给 [`signature`] 即可；同一个位置要稳定。
//! - `traceback.format_exception` 生成的堆栈文本：由调用方传入（[`ExcInfo::stack`]）。
//!
//! 3.0 新增（旧版没有）：「自动处理」的纯判断——哪些噪音不算 Bug、哪条未处理的 Bug
//! 该自动关闭（[`auto_resolve_reason`]）。I/O 在 gate-db（查询 / 更新）与 gate-server（`bugs.rs`、维护循环）。
//!
//! 特征码 = `sha1(basis)` 的前 12 个十六进制字符。SHA-1 在这里只是去重用的指纹，不涉及安全，
//! 所以用一个自带的小实现（工作区依赖里没有 sha1），上线期间与 `hashlib.sha1` 逐条对拍过。

use crate::pyc;

pub const LEVELS: [&str; 2] = ["error", "warn"];
pub const DETAIL_MAX: usize = 4000;
/// 同一个 bug 的完整堆栈多久只打印一次（秒）。
pub const PRINT_STACK_INTERVAL: f64 = 600.0;

// ---------------------------------------------------------------- SHA-1

/// SHA-1（RFC 3174），返回 40 位小写十六进制。
fn sha1_hex(data: &[u8]) -> String {
    let mut h: [u32; 5] = [
        0x6745_2301,
        0xEFCD_AB89,
        0x98BA_DCFE,
        0x1032_5476,
        0xC3D2_E1F0,
    ];
    let mut msg = data.to_vec();
    let bit_len = (data.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());
    for block in msg.as_chunks::<64>().0 {
        let mut w = [0u32; 80];
        for (i, word) in block.as_chunks::<4>().0.iter().enumerate() {
            w[i] = u32::from_be_bytes(*word);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for (i, wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | (!b & d), 0x5A82_7999),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let t = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = t;
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }
    h.iter().map(|x| format!("{x:08x}")).collect()
}

// ---------------------------------------------------------------- 特征码

/// `_NUM.sub('#', title)[:200]`，`_NUM = r"\d+(\.\d+)?"`：把数字（含一个小数点）折叠成 `#`，
/// 让「上游返回 502 / 503」「耗时 1.5s / 2.5s」归到同一条 bug。
pub fn normalize_title(title: &str) -> String {
    let chars: Vec<char> = title.chars().collect();
    let mut out = String::with_capacity(title.len());
    let mut i = 0;
    while i < chars.len() {
        if crate::pyext::is_nd(chars[i]) {
            while i < chars.len() && crate::pyext::is_nd(chars[i]) {
                i += 1;
            }
            // 可选的小数部分：'.' 后至少一位数字
            if i + 1 < chars.len() && chars[i] == '.' && crate::pyext::is_nd(chars[i + 1]) {
                i += 1;
                while i < chars.len() && crate::pyext::is_nd(chars[i]) {
                    i += 1;
                }
            }
            out.push('#');
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    pyc::take_chars(&out, 200).to_string()
}

/// 带异常时特征码只看「来源 + 异常类型 + 代码位置」。
#[derive(Debug, Clone, Copy)]
pub struct ExcKey<'a> {
    /// `type(exc).__name__`
    pub type_name: &'a str,
    /// `_app_frame(exc)`：`文件名.py:函数名`，没有本项目的帧时为空串。
    pub app_frame: &'a str,
}

/// `signature(source, exc, title)`：12 位十六进制特征码。
/// 有异常：`sha1("{source}|{类型}|{位置}")`；没有：`sha1("{source}|{折叠数字后的标题[:200]}")`。
pub fn signature(source: &str, exc: Option<ExcKey<'_>>, title: &str) -> String {
    let basis = match exc {
        Some(e) => format!("{source}|{}|{}", e.type_name, e.app_frame),
        None => format!("{source}|{}", normalize_title(title)),
    };
    sha1_hex(basis.as_bytes())[..12].to_string()
}

// ---------------------------------------------------------------- 事件

/// 异常信息（Rust 侧由调用方填）。
#[derive(Debug, Clone, Copy)]
pub struct ExcInfo<'a> {
    /// `type(exc).__name__`
    pub type_name: &'a str,
    /// `str(exc)`
    pub message: &'a str,
    /// 代码位置，见 [`ExcKey::app_frame`]。
    pub app_frame: &'a str,
    /// 完整堆栈文本（`"".join(traceback.format_exception(...))`）。
    pub stack: &'a str,
}

/// `capture(...)` 的入参。
#[derive(Debug, Clone, Copy)]
pub struct Capture<'a> {
    pub source: &'a str,
    pub exc: Option<ExcInfo<'a>>,
    pub title: &'a str,
    pub detail: &'a str,
    pub path: &'a str,
    pub key_id: Option<i64>,
    pub rid: &'a str,
    /// 不在 [`LEVELS`] 里的一律当 `error`。
    pub level: &'a str,
}

/// 入库 / 保留在内存里的事件（字段顺序即旧版 dict 的键顺序）。
#[derive(Debug, Clone, PartialEq)]
pub struct ErrorEvent {
    pub sig: String,
    pub source: String,
    pub level: String,
    pub title: String,
    pub detail: String,
    pub path: String,
    pub key_id: Option<i64>,
    pub rid: String,
    pub ts: f64,
}

/// `capture` 的纯计算结果。
#[derive(Debug, Clone, PartialEq)]
pub struct Captured {
    pub event: ErrorEvent,
    /// 一行日志：`[bug] {level} {source} sig={sig} rid={rid或-} {title[:160]}`。
    pub log_line: String,
    /// 若本次要打印完整堆栈（带异常且距上次打印超过 [`PRINT_STACK_INTERVAL`]），要打印的文本。
    pub stack_text: String,
}

/// 同一个 bug 的堆栈 10 分钟内只打印一次：`now - last.get(sig, 0) > 600`。
pub fn should_print_stack(now: f64, last_printed: Option<f64>) -> bool {
    now - last_printed.unwrap_or(0.0) > PRINT_STACK_INTERVAL
}

/// 事件字段的截断：保留末尾（堆栈的最后几行最有用）。
fn tail_chars(s: &str, n: usize) -> &str {
    pyc::last_chars(s, n)
}

/// `Tracker.capture` 里不碰 I/O 的部分：规整 level、拼标题 / 详情、算特征码、按字段上限截断。
pub fn build_event(c: &Capture<'_>, now: f64) -> Captured {
    let level = if LEVELS.contains(&c.level) {
        c.level
    } else {
        "error"
    };
    let mut title = c.title.to_string();
    let mut detail = c.detail.to_string();
    if let Some(e) = &c.exc {
        if title.is_empty() {
            title = format!("{}: {}", e.type_name, pyc::take_chars(e.message, 200));
        }
        detail = format!(
            "{}{}",
            if detail.is_empty() {
                String::new()
            } else {
                format!("{detail}\n")
            },
            e.stack
        );
    }
    let sig = signature(
        c.source,
        c.exc.map(|e| ExcKey {
            type_name: e.type_name,
            app_frame: e.app_frame,
        }),
        &title,
    );
    let rid_log = if c.rid.is_empty() { "-" } else { c.rid };
    let log_line = format!(
        "[bug] {level} {} sig={sig} rid={rid_log} {}",
        c.source,
        pyc::take_chars(&title, 160)
    );
    let stack_text = tail_chars(&detail, DETAIL_MAX).to_string();
    let event = ErrorEvent {
        sig,
        source: pyc::take_chars(c.source, 60).to_string(),
        level: level.to_string(),
        title: pyc::take_chars(&title, 300).to_string(),
        detail: stack_text.clone(),
        path: pyc::take_chars(c.path, 200).to_string(),
        key_id: c.key_id,
        rid: pyc::take_chars(c.rid, 16).to_string(),
        ts: now,
    };
    Captured {
        event,
        log_line,
        stack_text,
    }
}

// ---------------------------------------------------------------- 自动处理（3.0 新增）

/// 注意级（warn）的 Bug 多久没再出现就自动关闭（秒）：24 小时。
pub const STALE_WARN_SECS: f64 = 24.0 * 3600.0;
/// 错误级（error）的 Bug 多久没再出现就自动关闭（秒）：7 天。
pub const STALE_ERROR_SECS: f64 = 7.0 * 86400.0;
/// 自动处理最多多久跑一次（秒）。
pub const TRIAGE_INTERVAL: f64 = 600.0;
/// 本站前端脚本的路径特征：3.0 的页面只加载 `/_owl/` 下的文件，CSP 又不允许内联脚本，
/// 所以带出错文件的脚本报错，文件不在这个路径下就一定不是我们的代码。
pub const OWN_SCRIPT_PATH: &str = "/_owl/";

/// 自动关闭的原因。落库的是 [`AutoReason::code`]（`error_events.auto_resolved`），后台页面按码显示文案。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoReason {
    /// 浏览器网络失败（断网 / 切后台 / 请求被取消）：不是我们的缺陷。
    Network,
    /// 上游（NovelAI）返回 5xx 或连不上：已在用量日志和「上游」页里，不是我们的缺陷。
    Upstream,
    /// 不是本站脚本出的错（旧版缓存页、浏览器插件、手机自带浏览器注入）。
    ForeignScript,
    /// 注意级超过 [`STALE_WARN_SECS`] 没再出现。
    StaleWarn,
    /// 错误级超过 [`STALE_ERROR_SECS`] 没再出现。
    StaleError,
}

impl AutoReason {
    pub const ALL: [AutoReason; 5] = [
        AutoReason::Network,
        AutoReason::Upstream,
        AutoReason::ForeignScript,
        AutoReason::StaleWarn,
        AutoReason::StaleError,
    ];

    pub fn code(self) -> &'static str {
        match self {
            AutoReason::Network => "network",
            AutoReason::Upstream => "upstream",
            AutoReason::ForeignScript => "foreign_script",
            AutoReason::StaleWarn => "stale_warn",
            AutoReason::StaleError => "stale_error",
        }
    }
}

/// 浏览器网络失败的报错文案（小写比较）：断网、切后台、页面跳转时被取消的请求都会这样报。
/// 前端 `web/src/lib/api.ts` 的 `isNetworkFailure` 用同一份清单。
const NETWORK_NEEDLES: [&str; 10] = [
    "failed to fetch",
    "networkerror",
    "load failed",
    "network error",
    "the network connection was lost",
    "aborterror",
    "the operation was aborted",
    "cancelled",
    "canceled",
    "已取消",
];

/// 报错文案是不是浏览器的网络失败（不分大小写，包含即可）。
pub fn is_network_failure(msg: &str) -> bool {
    let m = msg.to_lowercase();
    NETWORK_NEEDLES.iter().any(|n| m.contains(n))
}

/// Chrome 对未捕获异常的报错文案以 `Uncaught ` 开头。
pub fn is_uncaught(msg: &str) -> bool {
    msg.starts_with("Uncaught ")
}

/// 一条网页报错是不是出自别人的脚本。
/// - 上报里带了 `filename`（出错脚本的地址，3.0 前端取自 `ErrorEvent.filename`）：必须在 `/_owl/` 下；
/// - 没带（Promise 拒绝，或还没刷新的老页面）：只查 `Uncaught …` 这种脚本错误，`src` 或堆栈里得有 `/_owl/`；
///   其余消息交给调用方原有的「堆栈里没有一帧来自本站」判断。
pub fn is_foreign_script_report(msg: &str, filename: Option<&str>, src: &str, stack: &str) -> bool {
    match filename {
        Some(f) => !f.contains(OWN_SCRIPT_PATH),
        None => {
            is_uncaught(msg) && !(src.contains(OWN_SCRIPT_PATH) || stack.contains(OWN_SCRIPT_PATH))
        }
    }
}

/// 上游故障落成的 Bug 标题（错误账本把上游 5xx 记成 `gate` / `upstream` 来源，标题是「状态码 + 给成员的文案」）。
/// 新的上游故障不再记 Bug；这条规则只用来清理旧行。
fn is_upstream_fault_row(source: &str, title: &str) -> bool {
    const MARKS: [&str; 4] = ["上游", "Vibe 编码", "图片工具", "图片流连接中断"];
    matches!(source, "gate" | "upstream") && MARKS.iter().any(|m| title.contains(m))
}

/// 已落库的网页报错是不是别人的脚本：jQuery 的 `$`（3.0 前端从不用）一律算；`Uncaught …` 的详情
/// （`{src}:{line}\n{UA}\n{堆栈}`）里没有 `/_owl/` 也算。
fn is_foreign_script_row(source: &str, title: &str, detail: &str) -> bool {
    source.starts_with("web:")
        && (title.contains("$ is not defined")
            || (is_uncaught(title) && !detail.contains(OWN_SCRIPT_PATH)))
}

/// 一条还没处理的 Bug（自动处理的输入）。
#[derive(Debug, Clone, Copy)]
pub struct OpenBug<'a> {
    pub source: &'a str,
    pub level: &'a str,
    pub title: &'a str,
    pub detail: &'a str,
    pub last_seen: f64,
}

/// 这条未处理的 Bug 该不该自动关闭、为什么。先看是不是「不算 Bug」的噪音（立刻关），再看多久没出现。
/// 级别不是 `warn` 的一律按 `error` 算（与 [`build_event`] 一致）；`last_seen` 在未来（时钟回拨）不关。
pub fn auto_resolve_reason(b: &OpenBug<'_>, now: f64) -> Option<AutoReason> {
    if b.source.starts_with("web:") && is_network_failure(b.title) {
        return Some(AutoReason::Network);
    }
    if is_upstream_fault_row(b.source, b.title) {
        return Some(AutoReason::Upstream);
    }
    if is_foreign_script_row(b.source, b.title, b.detail) {
        return Some(AutoReason::ForeignScript);
    }
    let idle = now - b.last_seen;
    if b.level == "warn" {
        (idle >= STALE_WARN_SECS).then_some(AutoReason::StaleWarn)
    } else {
        (idle >= STALE_ERROR_SECS).then_some(AutoReason::StaleError)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bug<'a>(source: &'a str, level: &'a str, title: &'a str, last_seen: f64) -> OpenBug<'a> {
        OpenBug {
            source,
            level,
            title,
            detail: "",
            last_seen,
        }
    }

    #[test]
    fn network_failures_are_recognised_case_insensitively() {
        for m in [
            "Failed to fetch",
            "live: Failed to fetch",
            "NetworkError when attempting to fetch resource.",
            "TypeError: Load failed",
            "A network error occurred.",
            "The network connection was lost.",
            "AbortError: The operation was aborted.",
            "fetch cancelled",
            "请求已取消",
            "FAILED TO FETCH",
        ] {
            assert!(is_network_failure(m), "{m}");
        }
        for m in [
            "TypeError: x is undefined",
            "ReferenceError: y",
            "",
            "加载失败",
        ] {
            assert!(!is_network_failure(m), "{m}");
        }
    }

    #[test]
    fn foreign_script_reports() {
        let own = "https://gate.example.top/_owl/assets/app-abc.js";
        // 带 filename：只看 filename
        assert!(!is_foreign_script_report(
            "Uncaught Error: x",
            Some(own),
            "",
            ""
        ));
        assert!(is_foreign_script_report(
            "Uncaught ReferenceError: $ is not defined",
            Some("https://gate.example.top/"),
            "",
            ""
        ));
        assert!(is_foreign_script_report(
            "ReferenceError: $ is not defined",
            Some(""),
            "",
            ""
        ));
        assert!(is_foreign_script_report(
            "x",
            Some("chrome-extension://abc/inject.js"),
            own,
            own
        ));
        // 没带 filename：Uncaught 要在 src / 堆栈里找到本站文件，其余消息不在这里判断
        assert!(is_foreign_script_report(
            "Uncaught Error: x",
            None,
            "/",
            "at https://gate.example.top/:1:2"
        ));
        assert!(!is_foreign_script_report(
            "Uncaught Error: x",
            None,
            own,
            ""
        ));
        assert!(!is_foreign_script_report(
            "Uncaught Error: x",
            None,
            "/",
            &format!("at f ({own}:1:2)")
        ));
        assert!(!is_foreign_script_report(
            "TypeError: x is undefined",
            None,
            "",
            ""
        ));
    }

    #[test]
    fn noise_is_resolved_immediately_regardless_of_age() {
        let now = 1_000_000.0;
        let fresh = now - 5.0;
        let reason =
            |source, level, title| auto_resolve_reason(&bug(source, level, title, fresh), now);
        // 浏览器网络失败（2.x 页面带 `live:` 前缀）
        assert_eq!(
            reason("web:landing", "warn", "live: Failed to fetch"),
            Some(AutoReason::Network)
        );
        assert_eq!(
            reason(
                "web:admin",
                "warn",
                "NetworkError when attempting to fetch resource."
            ),
            Some(AutoReason::Network)
        );
        // 同样的字样出在服务端来源（比如后台任务的错误消息）不算噪音
        assert_eq!(
            reason("maintenance:official", "warn", "request cancelled"),
            None
        );
        // 上游故障
        assert_eq!(
            reason(
                "gate",
                "warn",
                "500 上游服务暂时不可用，请稍后重试（上游状态 500）"
            ),
            Some(AutoReason::Upstream)
        );
        assert_eq!(
            reason(
                "upstream",
                "warn",
                "502 Vibe 编码连接失败，未记费；请勿自动重试"
            ),
            Some(AutoReason::Upstream)
        );
        // 我们自己的 500 不动
        assert_eq!(
            reason("request", "error", "InternalError: 数据库已损坏"),
            None
        );
        assert_eq!(reason("gate", "warn", "500 请求体解析失败"), None);
        // 非本站脚本
        assert_eq!(
            reason(
                "web:landing",
                "warn",
                "Uncaught ReferenceError: $ is not defined"
            ),
            Some(AutoReason::ForeignScript)
        );
        assert_eq!(
            reason("web:other", "warn", "ReferenceError: $ is not defined"),
            Some(AutoReason::ForeignScript)
        );
    }

    #[test]
    fn own_script_errors_are_kept() {
        let now = 1_000_000.0;
        let mut b = bug(
            "web:landing",
            "warn",
            "Uncaught TypeError: x is undefined",
            now - 60.0,
        );
        b.detail = "https://gate.example.top/_owl/assets/app-abc.js:7\nMozilla/5.0 (synthetic)\nTypeError: x";
        assert_eq!(auto_resolve_reason(&b, now), None);
        // 没有 /_owl/ 的 Uncaught（旧版页面 / 插件）立刻关
        b.detail = "/:7\nMozilla/5.0 (synthetic)\nTypeError: x";
        assert_eq!(
            auto_resolve_reason(&b, now),
            Some(AutoReason::ForeignScript)
        );
        // 不是 Uncaught 的普通报错（比如 Promise 拒绝）不凭文案判断
        let mut c = bug(
            "web:landing",
            "warn",
            "TypeError: x is undefined",
            now - 60.0,
        );
        c.detail = "/:0\nMozilla/5.0 (synthetic)\n";
        assert_eq!(auto_resolve_reason(&c, now), None);
    }

    #[test]
    fn stale_bugs_close_by_level() {
        let now = 10_000_000.0;
        let at = |level, ago: f64| {
            auto_resolve_reason(
                &bug("maintenance:disk", level, "磁盘读数失败", now - ago),
                now,
            )
        };
        assert_eq!(at("warn", 24.0 * 3600.0 - 1.0), None);
        assert_eq!(at("warn", 24.0 * 3600.0), Some(AutoReason::StaleWarn));
        assert_eq!(at("warn", 3.0 * 86400.0), Some(AutoReason::StaleWarn));
        // 错误级要等 7 天
        assert_eq!(at("error", 2.0 * 86400.0), None);
        assert_eq!(at("error", 7.0 * 86400.0 - 1.0), None);
        assert_eq!(at("error", 7.0 * 86400.0), Some(AutoReason::StaleError));
        // 不认识的级别按 error 算
        assert_eq!(at("weird", 3.0 * 86400.0), None);
        assert_eq!(at("weird", 8.0 * 86400.0), Some(AutoReason::StaleError));
        // 时钟回拨：last_seen 在未来，不关
        assert_eq!(at("warn", -3600.0), None);
    }

    #[test]
    fn sha1_known_vectors() {
        assert_eq!(sha1_hex(b""), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        assert_eq!(sha1_hex(b"abc"), "a9993e364706816aba3e25717850c26c9cd0d89d");
        assert_eq!(
            sha1_hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "84983e441c3bd26ebaae4aa1f95129e5e54670f1"
        );
    }

    #[test]
    fn numbers_fold_to_hash() {
        assert_eq!(
            normalize_title("上游 502 耗时 1.5s 与 2.50.3"),
            "上游 # 耗时 #s 与 #.#"
        );
        assert_eq!(normalize_title("a1b22.c3."), "a#b#.c#.");
        assert_eq!(normalize_title("１２.５"), "#");
        assert_eq!(normalize_title("x"), "x");
    }

    #[test]
    fn unknown_level_counts_as_error() {
        let c = Capture {
            source: "background",
            exc: None,
            title: "维护循环失败 3 次",
            detail: "",
            path: "",
            key_id: None,
            rid: "abcd1234",
            level: "weird",
        };
        let got = build_event(&c, 100.0);
        assert_eq!(got.event.level, "error");
    }
}
