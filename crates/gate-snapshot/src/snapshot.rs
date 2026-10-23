//! 快照：把当前 Rust 的输出存成纯文本，改动时在 git diff 里一眼看出哪一例变了。
//!
//! 文件是 `<crate>/tests/snapshots/<套件>/<名字>.jsonl`（`check` / `Suite::open` 的 `name` 写成 `"套件/名字"`），
//! 每行一个用例：`{"in":...,"out":...}`。
//! - 紧凑序列化，键按插入顺序（workspace 已开 serde_json 的 `preserve_order` 和 `float_roundtrip`），非 ASCII 字符原样输出；
//! - 沿用夹具的编码约定：非有限浮点 `{"$f":"inf"|"-inf"|"nan"}`、bytes `{"$b":"<base64>"}`、非字符串键 `{"$map":[[k,v],...]}`；
//! - 同一个函数的用例连在一起，组内保持原来的顺序。
//!
//! `Case` 与旧夹具共用同一个类型，`args()` / `kwarg(name)` / `arg(i)` / `get(k)` 读的是 `"in"`，迁移时测试里的取数代码不用改。
//!
//! 默认模式逐行比较 `serde_json::to_string(实际输出)` 与存下来的 `out` 串：int 和 float 不混、浮点逐位、键顺序一致。
//! 失败最多列 10 条（行号、第一处差异的 JSON 路径、截断后的输入），并给出更新命令。`f` 返回 `None` 在默认模式算失败。
//!
//! 更新模式（`UPDATE_SNAPSHOT=1`）原子重写文件（写临时文件再 rename）：
//! - `f` 返回 `None` 表示「这个输入不再适用」，该行被删掉；
//! - 新增用例：手工追加一行 `{"in":...,"out":null}`，再更新一次就会被填上；
//! - 最后在 stderr 打印「改动 x 条、删除 y 条、共 z 条」，评审时核对数字。
//!
//! 设置了 `CI` 环境变量时如果又带 `UPDATE_SNAPSHOT`，直接 panic：CI 永远不改写快照。

use crate::{Case, diff};
use serde_json::Value;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// 触发更新模式的环境变量。
pub const UPDATE_ENV: &str = "UPDATE_SNAPSHOT";

/// 失败时最多列出的条数。
const MAX_LISTED: usize = 10;
/// 失败信息里输入最多显示的字符数。
const INPUT_PREVIEW_CHARS: usize = 200;
/// 失败信息里差异说明最多显示的字符数。
const DIFF_PREVIEW_CHARS: usize = 300;

/// 快照根目录：`<crate>/tests/snapshots`。
pub fn dir(manifest_dir: &str) -> PathBuf {
    Path::new(manifest_dir).join("tests").join("snapshots")
}

/// `<crate>/tests/snapshots/<name>.jsonl`；`name` 形如 `"quota/decide"`。
pub fn path(manifest_dir: &str, name: &str) -> PathBuf {
    dir(manifest_dir).join(format!("{name}.jsonl"))
}

/// 运行模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// 默认：只比较，不写文件。
    Check,
    /// `UPDATE_SNAPSHOT=1`：用当前输出重写文件。
    Update,
}

impl Mode {
    /// 由「是否在 CI」和「是否要求更新」决定模式；CI 里要求更新是错误。
    pub fn resolve(ci: bool, update: bool) -> Result<Mode, String> {
        match (ci, update) {
            (true, true) => Err(format!(
                "检测到 CI 环境变量，同时又设置了 {UPDATE_ENV}：CI 里永远不改写快照。\
                 请去掉 {UPDATE_ENV}，在本地更新快照并把改动提交上来。"
            )),
            (false, true) => Ok(Mode::Update),
            (_, false) => Ok(Mode::Check),
        }
    }

    /// 读环境变量：`CI` 只要设置了就算在 CI；`UPDATE_SNAPSHOT` 设置成空串、`0`、`false` 不算要求更新。
    /// CI 里又要求更新时 panic。
    #[track_caller]
    pub fn from_env() -> Mode {
        let ci = std::env::var_os("CI").is_some();
        let update = flag_is_on(std::env::var_os(UPDATE_ENV).as_deref());
        Mode::resolve(ci, update).unwrap_or_else(|e| panic!("{e}"))
    }
}

/// 开关型环境变量：没设置、空串、`0`、`false` 都算关。
fn flag_is_on(value: Option<&std::ffi::OsStr>) -> bool {
    value.is_some_and(|v| {
        let v = v.to_string_lossy();
        let v = v.trim();
        !(v.is_empty() || v == "0" || v.eq_ignore_ascii_case("false"))
    })
}

/// 更新模式的结果统计。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Summary {
    /// 输出和原来不同的行数（含 `"out": null` 被填充的行）。
    pub changed: usize,
    /// 被删掉的行数（`f` 返回了 `None`）。
    pub deleted: usize,
    /// 重写之后文件里的总行数。
    pub total: usize,
}

impl Summary {
    fn line(&self, name: &str) -> String {
        format!(
            "[snapshot] {name}: 改动 {} 条、删除 {} 条、共 {} 条",
            self.changed, self.deleted, self.total
        )
    }
}

/// 逐个用例比较 / 更新，函数式套件用这个：
/// `check(env!("CARGO_MANIFEST_DIR"), "quota/decide", |c| Some(run(c)))`。
#[track_caller]
pub fn check(manifest_dir: &str, name: &str, f: impl FnMut(&Case) -> Option<Value>) {
    let mode = Mode::from_env();
    match run(manifest_dir, name, mode, f) {
        Ok(summary) => report(name, summary),
        Err(msg) => panic!("{msg}"),
    }
}

/// 更新模式的统计打到 stderr。直接写 stderr 而不用 `eprintln!`：不被测试框架的输出捕获吞掉，更新时一眼能看到数字。
fn report(name: &str, summary: Option<Summary>) {
    if let Some(summary) = summary {
        let _ = writeln!(std::io::stderr(), "{}", summary.line(name));
    }
}

/// `check` 的实现：失败信息放在 `Err` 里，更新模式返回统计。
pub(crate) fn run(
    manifest_dir: &str,
    name: &str,
    mode: Mode,
    mut f: impl FnMut(&Case) -> Option<Value>,
) -> Result<Option<Summary>, String> {
    let mut suite = Suite::open_with(manifest_dir, name, mode);
    let cases = suite.cases();
    for (i, case) in cases.iter().enumerate() {
        suite.record(i, f(case));
    }
    suite.conclude()
}

/// 异步或多步共享状态的套件（admin、repo）手动驱动：
/// `open` → 遍历 `cases()` 自己跑 → 每条 `record(i, 输出)` → `finish`。
pub struct Suite {
    name: String,
    path: PathBuf,
    mode: Mode,
    update_cmd: String,
    cases: Arc<[Case]>,
    stored: Vec<Value>,
    recorded: Vec<Option<Option<Value>>>,
}

impl Suite {
    /// 读快照文件；模式由环境变量决定（CI 里带 `UPDATE_SNAPSHOT` 会 panic）。
    #[track_caller]
    pub fn open(manifest_dir: &str, name: &str) -> Suite {
        Suite::open_with(manifest_dir, name, Mode::from_env())
    }

    #[track_caller]
    pub(crate) fn open_with(manifest_dir: &str, name: &str, mode: Mode) -> Suite {
        let path = path(manifest_dir, name);
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| {
            panic!(
                "读不到快照 {}: {e}\n新建快照：手工写若干行 {{\"in\": ..., \"out\": null}}，再用 {UPDATE_ENV}=1 填充输出。",
                path.display()
            )
        });
        let (cases, stored) = parse(&path, &text);
        assert!(
            !cases.is_empty(),
            "快照 {} 里一条用例都没有（被误清空了？）",
            path.display()
        );
        let n = cases.len();
        Suite {
            name: name.to_string(),
            update_cmd: update_command(manifest_dir),
            path,
            mode,
            cases: cases.into(),
            stored,
            recorded: vec![None; n],
        }
    }

    /// 全部用例（`Case::raw` 是 `"in"`）。返回共享引用，遍历时可以同时调用 `record`。
    pub fn cases(&self) -> Arc<[Case]> {
        Arc::clone(&self.cases)
    }

    /// 记下第 `i` 条（`cases()` 里的下标）的实际输出；`None` 表示「这个输入不再适用」。每条只能记一次。
    #[track_caller]
    pub fn record(&mut self, i: usize, out: Option<Value>) {
        let n = self.recorded.len();
        let slot = self
            .recorded
            .get_mut(i)
            .unwrap_or_else(|| panic!("record({i}) 越界：快照 {} 只有 {n} 条", self.name));
        assert!(
            slot.is_none(),
            "快照 {} 的第 {i} 条（文件第 {} 行）被记录了两次",
            self.name,
            self.cases[i].line
        );
        *slot = Some(out);
    }

    /// 默认模式：有不一致就 panic；更新模式：重写文件并在 stderr 打印统计。
    #[track_caller]
    pub fn finish(self) {
        let name = self.name.clone();
        match self.conclude() {
            Ok(summary) => report(&name, summary),
            Err(msg) => panic!("{msg}"),
        }
    }

    /// `finish` 的实现：默认模式返回 `Ok(None)` 或 `Err(失败信息)`，更新模式返回统计。
    pub(crate) fn conclude(self) -> Result<Option<Summary>, String> {
        match self.mode {
            Mode::Check => self.verify().map(|()| None),
            Mode::Update => self.rewrite().map(Some),
        }
    }

    fn verify(&self) -> Result<(), String> {
        let mut bad = 0usize;
        let mut listed = Vec::new();
        for (i, case) in self.cases.iter().enumerate() {
            let problem = match &self.recorded[i] {
                None => Some("没有被执行（漏了 record）".to_string()),
                Some(None) => Some("返回了 None（输入不再适用？更新模式会删掉这一行）".to_string()),
                Some(Some(actual)) => compare(actual, &self.stored[i]),
            };
            if let Some(p) = problem {
                bad += 1;
                if listed.len() < MAX_LISTED {
                    listed.push(format!(
                        "  第 {} 行：{p}\n    输入：{}",
                        case.line,
                        clip(&to_compact(&case.raw), INPUT_PREVIEW_CHARS)
                    ));
                }
            }
        }
        if bad == 0 {
            return Ok(());
        }
        let mut msg = format!(
            "快照 {}（{}）有 {bad} 条与当前输出不一致（共 {} 条）：\n{}",
            self.name,
            self.path.display(),
            self.cases.len(),
            listed.join("\n")
        );
        if bad > listed.len() {
            msg.push_str(&format!("\n  ……另有 {} 条未列出", bad - listed.len()));
        }
        msg.push_str(&format!(
            "\n确认是有意的改动后更新快照，并审阅 git diff：\n  {}",
            self.update_cmd
        ));
        Err(msg)
    }

    fn rewrite(&self) -> Result<Summary, String> {
        let mut text = String::new();
        let (mut changed, mut deleted, mut total) = (0, 0, 0);
        for (i, case) in self.cases.iter().enumerate() {
            let new_out = match &self.recorded[i] {
                // 没被执行的行原样保留，不替用户做删除的决定
                None => &self.stored[i],
                Some(None) => {
                    deleted += 1;
                    continue;
                }
                Some(Some(out)) => {
                    if to_compact(out) != to_compact(&self.stored[i]) {
                        changed += 1;
                    }
                    out
                }
            };
            text.push_str(&render_line(&case.raw, new_out));
            text.push('\n');
            total += 1;
        }
        // 内容没变就不动文件（不扰动 mtime）
        if std::fs::read_to_string(&self.path).ok().as_deref() != Some(text.as_str()) {
            write_atomic(&self.path, &text)
                .map_err(|e| format!("写快照 {} 失败: {e}", self.path.display()))?;
        }
        Ok(Summary {
            changed,
            deleted,
            total,
        })
    }
}

/// 一行的规范写法：`{"in":<in>,"out":<out>}`，紧凑、键按插入顺序、非 ASCII 原样。
pub(crate) fn render_line(input: &Value, out: &Value) -> String {
    format!(
        "{{\"in\":{},\"out\":{}}}",
        to_compact(input),
        to_compact(out)
    )
}

pub(crate) fn to_compact(v: &Value) -> String {
    serde_json::to_string(v).expect("serde_json::Value 总能序列化")
}

/// 原子写：同目录写临时文件，再 rename 覆盖。
pub(crate) fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = parent.join(format!(".{file_name}.tmp-{}", std::process::id()));
    let result = (|| {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// 解析快照文件；格式不对直接 panic（带文件和行号）。
#[track_caller]
fn parse(path: &Path, text: &str) -> (Vec<Case>, Vec<Value>) {
    let mut cases = Vec::new();
    let mut stored = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let at = format!("{}:{}", path.display(), i + 1);
        let value: Value =
            serde_json::from_str(line).unwrap_or_else(|e| panic!("{at} 不是合法 JSON: {e}"));
        let Value::Object(mut obj) = value else {
            panic!("{at} 每行必须是 {{\"in\": ..., \"out\": ...}} 对象");
        };
        let input = obj
            .remove("in")
            .unwrap_or_else(|| panic!("{at} 缺少 \"in\""));
        let out = obj.remove("out").unwrap_or(Value::Null);
        if let Some(extra) = obj.keys().next() {
            panic!("{at} 多出键 {extra:?}：每行只能有 \"in\" 和 \"out\"");
        }
        cases.push(Case {
            line: i + 1,
            raw: input,
        });
        stored.push(out);
    }
    (cases, stored)
}

/// 比较实际输出和存下来的 `out`：序列化文本逐字符相同才算一致。不一致时给出第一处差异。
fn compare(actual: &Value, expected: &Value) -> Option<String> {
    let a = to_compact(actual);
    let e = to_compact(expected);
    if a == e {
        return None;
    }
    let mut msg = match diff(actual, expected) {
        Some(d) => clip(&d, DIFF_PREVIEW_CHARS),
        // 极少数情况（如对象里混着 `$f` 键）diff 认为相同，退回到文本比较
        None => first_text_difference(&a, &e),
    };
    if expected.is_null() {
        msg.push_str("（期望是 null：新增的用例要先用更新模式填充）");
    }
    Some(msg)
}

fn first_text_difference(a: &str, e: &str) -> String {
    let at = a
        .chars()
        .zip(e.chars())
        .position(|(x, y)| x != y)
        .unwrap_or_else(|| a.chars().count().min(e.chars().count()));
    let window = |s: &str| -> String { s.chars().skip(at.saturating_sub(20)).take(60).collect() };
    format!(
        "序列化文本从第 {at} 个字符起不同：实际 …{}… ≠ 期望 …{}…",
        window(a),
        window(e)
    )
}

/// 按字符数截断，超出部分用 `…` 标记。
fn clip(s: &str, max_chars: usize) -> String {
    let n = s.chars().count();
    if n <= max_chars {
        return s.to_string();
    }
    let head: String = s.chars().take(max_chars).collect();
    format!("{head}…（共 {n} 字符）")
}

/// 更新命令提示：`UPDATE_SNAPSHOT=1 cargo test -p <crate> --test <测试文件名>`。
fn update_command(manifest_dir: &str) -> String {
    let krate = Path::new(manifest_dir)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "<crate>".to_string());
    let target = std::env::current_exe().ok().and_then(|exe| {
        exe.file_stem()
            .and_then(|s| s.to_str())
            .and_then(test_target_from_stem)
    });
    match target {
        Some(t) => format!("{UPDATE_ENV}=1 cargo test -p {krate} --test {t}"),
        None => format!("{UPDATE_ENV}=1 cargo test -p {krate}"),
    }
}

/// cargo 把集成测试编成 `<名字>-<16 位十六进制>`，去掉后缀得到 `--test` 要的名字。
fn test_target_from_stem(stem: &str) -> Option<String> {
    let (name, hash) = stem.rsplit_once('-')?;
    (!name.is_empty() && hash.len() == 16 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// 临时的「crate 目录」：`<tmp>/<唯一名>/gate-demo`，快照放在它的 `tests/snapshots/` 下。
    struct Tmp {
        root: PathBuf,
        manifest: String,
    }

    impl Tmp {
        fn new() -> Tmp {
            static N: AtomicUsize = AtomicUsize::new(0);
            let root = std::env::temp_dir().join(format!(
                "gate-snapshot-test-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&root);
            let manifest = root.join("gate-demo");
            std::fs::create_dir_all(&manifest).unwrap();
            Tmp {
                root,
                manifest: manifest.to_string_lossy().into_owned(),
            }
        }
        fn file(&self, name: &str) -> PathBuf {
            path(&self.manifest, name)
        }
        fn write(&self, name: &str, lines: &[&str]) {
            let p = self.file(name);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            let mut text = lines.join("\n");
            text.push('\n');
            std::fs::write(p, text).unwrap();
        }
        fn read(&self, name: &str) -> String {
            std::fs::read_to_string(self.file(name)).unwrap()
        }
        fn check(
            &self,
            name: &str,
            mode: Mode,
            f: impl FnMut(&Case) -> Option<Value>,
        ) -> Result<Option<Summary>, String> {
            run(&self.manifest, name, mode, f)
        }
        /// 目录里有没有残留的临时文件。
        fn leftovers(&self, name: &str) -> Vec<String> {
            std::fs::read_dir(self.file(name).parent().unwrap())
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .filter(|n| n.contains(".tmp-"))
                .collect()
        }
    }

    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    /// 输入里 `n` 的两倍作为输出。
    fn double(c: &Case) -> Option<Value> {
        Some(json!(c.get("n")?.as_i64()? * 2))
    }

    #[test]
    fn check_passes_when_every_line_matches() {
        let t = Tmp::new();
        t.write(
            "demo/double",
            &[r#"{"in":{"n":1},"out":2}"#, r#"{"in":{"n":5},"out":10}"#],
        );
        let r = t.check("demo/double", Mode::Check, double).unwrap();
        assert_eq!(r, None);
        // 默认模式不动文件
        assert_eq!(
            t.read("demo/double"),
            "{\"in\":{\"n\":1},\"out\":2}\n{\"in\":{\"n\":5},\"out\":10}\n"
        );
    }

    #[test]
    fn check_failure_reports_line_path_input_and_the_update_command() {
        let t = Tmp::new();
        t.write(
            "demo/obj",
            &[
                r#"{"in":{"n":1},"out":{"a":1,"b":{"c":[1,2]}}}"#,
                r#"{"in":{"n":2,"note":"输入很重要"},"out":{"a":1,"b":{"c":[1,2]}}}"#,
            ],
        );
        let err = t
            .check("demo/obj", Mode::Check, |c| {
                let c_val = if c.get("n")?.as_i64()? == 1 { 2 } else { 3 };
                Some(json!({"a": 1, "b": {"c": [1, c_val]}}))
            })
            .unwrap_err();
        assert!(err.contains("有 1 条与当前输出不一致（共 2 条）"), "{err}");
        assert!(err.contains("第 2 行"), "{err}");
        assert!(err.contains("$.b.c[1]"), "{err}");
        assert!(
            err.contains(r#"输入：{"n":2,"note":"输入很重要"}"#),
            "{err}"
        );
        assert!(!err.contains("第 1 行"), "{err}");
        assert!(
            err.contains("UPDATE_SNAPSHOT=1 cargo test -p gate-demo"),
            "{err}"
        );
    }

    #[test]
    fn check_failure_clips_long_inputs() {
        let t = Tmp::new();
        let long = "长".repeat(500);
        let line = format!(r#"{{"in":{{"s":"{long}"}},"out":1}}"#);
        t.write("demo/long", &[&line]);
        let err = t
            .check("demo/long", Mode::Check, |_| Some(json!(2)))
            .unwrap_err();
        assert!(err.contains("…（共 "), "{err}");
        assert!(!err.contains(&long), "输入应被截断");
    }

    #[test]
    fn check_lists_at_most_ten_failures() {
        let t = Tmp::new();
        let lines: Vec<String> = (0..13)
            .map(|i| format!(r#"{{"in":{{"n":{i}}},"out":0}}"#))
            .collect();
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        t.write("demo/many", &refs);
        let err = t
            .check("demo/many", Mode::Check, |c| {
                // n=0 的一行是对的，其余 12 行都不对
                Some(json!(c.get("n")?.as_i64()?))
            })
            .unwrap_err();
        assert!(
            err.contains("有 12 条与当前输出不一致（共 13 条）"),
            "{err}"
        );
        assert_eq!(err.matches("  第 ").count(), 10, "{err}");
        assert!(err.contains("另有 2 条未列出"), "{err}");
    }

    #[test]
    fn check_does_not_mix_int_and_float_or_key_order() {
        let t = Tmp::new();
        t.write("demo/types", &[r#"{"in":{},"out":{"a":1}}"#]);
        let err = t
            .check("demo/types", Mode::Check, |_| Some(json!({"a": 1.0})))
            .unwrap_err();
        assert!(err.contains("类型不同"), "{err}");

        t.write("demo/order", &[r#"{"in":{},"out":{"b":1,"a":2}}"#]);
        let err = t
            .check("demo/order", Mode::Check, |_| Some(json!({"a": 2, "b": 1})))
            .unwrap_err();
        assert!(err.contains("键顺序"), "{err}");
        // 顺序一样、浮点一样就是通过
        t.check("demo/order", Mode::Check, |_| Some(json!({"b": 1, "a": 2})))
            .unwrap();
        t.write("demo/float", &[r#"{"in":{},"out":0.30000000000000004}"#]);
        t.check("demo/float", Mode::Check, |_| Some(json!(0.1 + 0.2)))
            .unwrap();
        let err = t
            .check("demo/float", Mode::Check, |_| Some(json!(0.3)))
            .unwrap_err();
        assert!(err.contains("第 1 行"), "{err}");
    }

    #[test]
    fn special_floats_and_text_diff_fallback() {
        let t = Tmp::new();
        t.write(
            "demo/special",
            &[
                r#"{"in":{},"out":{"$f":"inf"}}"#,
                r#"{"in":{"x":1},"out":{"$f":"nan"}}"#,
            ],
        );
        t.check("demo/special", Mode::Check, |c| {
            Some(if c.get("x").is_some() {
                crate::f(f64::NAN)
            } else {
                crate::f(f64::INFINITY)
            })
        })
        .unwrap();
        let err = t
            .check("demo/special", Mode::Check, |_| {
                Some(crate::f(f64::NEG_INFINITY))
            })
            .unwrap_err();
        assert!(err.contains("有 2 条"), "{err}");
        // diff 看不出区别但文本不同（`$f` 对象里夹带别的键）时，退回文本比较
        let msg = compare(&json!({"$f": "inf", "x": 1}), &json!({"$f": "inf", "x": 2})).unwrap();
        assert!(msg.contains("序列化文本从第"), "{msg}");
    }

    #[test]
    fn none_is_a_failure_in_check_mode() {
        let t = Tmp::new();
        t.write("demo/none", &[r#"{"in":{"n":1},"out":2}"#]);
        let err = t.check("demo/none", Mode::Check, |_| None).unwrap_err();
        assert!(err.contains("返回了 None"), "{err}");
        assert!(err.contains("第 1 行"), "{err}");
    }

    #[test]
    fn null_out_fails_in_check_mode_with_a_hint() {
        let t = Tmp::new();
        t.write("demo/pending", &[r#"{"in":{"n":4},"out":null}"#]);
        let err = t.check("demo/pending", Mode::Check, double).unwrap_err();
        assert!(err.contains("期望是 null"), "{err}");
        // 函数本来就返回 null 的用例是正常的
        t.check("demo/pending", Mode::Check, |_| Some(Value::Null))
            .unwrap();
    }

    #[test]
    fn update_rewrites_changed_lines_and_counts_them() {
        let t = Tmp::new();
        t.write(
            "demo/update",
            &[
                r#"{"in":{"n":1},"out":2}"#,
                r#"{"in":{"n":2},"out":999}"#,
                r#"{"in":{"n":3},"out":6}"#,
            ],
        );
        let s = t
            .check("demo/update", Mode::Update, double)
            .unwrap()
            .unwrap();
        assert_eq!(
            s,
            Summary {
                changed: 1,
                deleted: 0,
                total: 3
            }
        );
        assert_eq!(
            t.read("demo/update"),
            "{\"in\":{\"n\":1},\"out\":2}\n{\"in\":{\"n\":2},\"out\":4}\n{\"in\":{\"n\":3},\"out\":6}\n"
        );
        assert!(t.leftovers("demo/update").is_empty());
        assert_eq!(
            s.line("demo/update"),
            "[snapshot] demo/update: 改动 1 条、删除 0 条、共 3 条"
        );
        // 更新之后默认模式通过
        t.check("demo/update", Mode::Check, double).unwrap();
    }

    #[test]
    fn update_counts_int_to_float_as_a_change() {
        let t = Tmp::new();
        t.write("demo/num", &[r#"{"in":{},"out":1}"#]);
        let s = t
            .check("demo/num", Mode::Update, |_| Some(json!(1.0)))
            .unwrap()
            .unwrap();
        assert_eq!(s.changed, 1);
        assert_eq!(t.read("demo/num"), "{\"in\":{},\"out\":1.0}\n");
    }

    #[test]
    fn update_deletes_the_line_when_f_returns_none() {
        let t = Tmp::new();
        t.write(
            "demo/del",
            &[
                r#"{"in":{"n":1},"out":2}"#,
                r#"{"in":{"n":-1},"out":0}"#,
                r#"{"in":{"n":3},"out":6}"#,
            ],
        );
        let s = t
            .check("demo/del", Mode::Update, |c| {
                let n = c.get("n")?.as_i64()?;
                (n > 0).then(|| json!(n * 2))
            })
            .unwrap()
            .unwrap();
        assert_eq!(
            s,
            Summary {
                changed: 0,
                deleted: 1,
                total: 2
            }
        );
        assert_eq!(
            t.read("demo/del"),
            "{\"in\":{\"n\":1},\"out\":2}\n{\"in\":{\"n\":3},\"out\":6}\n"
        );
    }

    #[test]
    fn update_fills_lines_whose_out_is_null() {
        let t = Tmp::new();
        t.write(
            "demo/fill",
            &[
                r#"{"in":{"n":1},"out":2}"#,
                r#"{"in":{"n":7},"out":null}"#,
                // 手工追加的行可以不写 out，也可以有多余空白
                r#"{ "in": {"n": 8} }"#,
            ],
        );
        let s = t.check("demo/fill", Mode::Update, double).unwrap().unwrap();
        assert_eq!(
            s,
            Summary {
                changed: 2,
                deleted: 0,
                total: 3
            }
        );
        assert_eq!(
            t.read("demo/fill"),
            "{\"in\":{\"n\":1},\"out\":2}\n{\"in\":{\"n\":7},\"out\":14}\n{\"in\":{\"n\":8},\"out\":16}\n"
        );
    }

    #[test]
    fn ci_forbids_update() {
        let e = Mode::resolve(true, true).unwrap_err();
        assert!(e.contains("CI") && e.contains("UPDATE_SNAPSHOT"), "{e}");
        assert_eq!(Mode::resolve(true, false), Ok(Mode::Check));
        assert_eq!(Mode::resolve(false, true), Ok(Mode::Update));
        assert_eq!(Mode::resolve(false, false), Ok(Mode::Check));
    }

    #[test]
    fn update_flag_values() {
        use std::ffi::OsStr;
        assert!(!flag_is_on(None));
        for off in ["", " ", "0", "false", "FALSE"] {
            assert!(!flag_is_on(Some(OsStr::new(off))), "{off:?}");
        }
        for on in ["1", "true", "yes"] {
            assert!(flag_is_on(Some(OsStr::new(on))), "{on:?}");
        }
    }

    /// 子进程里跑的探针：走真实的环境变量入口 `check`。只由 `env_vars_drive_the_public_entry_point` 调用。
    #[test]
    #[ignore = "由 env_vars_drive_the_public_entry_point 在子进程里运行"]
    fn env_probe() {
        let dir = std::env::var("GATE_SNAPSHOT_PROBE_DIR").expect("探针需要目录");
        check(&dir, "demo/probe", double);
    }

    #[test]
    fn env_vars_drive_the_public_entry_point() {
        let t = Tmp::new();
        t.write("demo/probe", &[r#"{"in":{"n":2},"out":999}"#]);
        let exe = std::env::current_exe().unwrap();
        let probe = |ci: Option<&str>, update: Option<&str>| {
            let mut cmd = std::process::Command::new(&exe);
            cmd.args([
                "--ignored",
                "--exact",
                "snapshot::tests::env_probe",
                "--nocapture",
            ])
            .env("GATE_SNAPSHOT_PROBE_DIR", &t.manifest)
            .env_remove("CI")
            .env_remove(UPDATE_ENV);
            if let Some(v) = ci {
                cmd.env("CI", v);
            }
            if let Some(v) = update {
                cmd.env(UPDATE_ENV, v);
            }
            let out = cmd.output().unwrap();
            let text = format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            (out.status.success(), text)
        };
        let stored = "{\"in\":{\"n\":2},\"out\":999}\n";

        // 默认：不一致就失败，文件不动
        let (ok, text) = probe(None, None);
        assert!(!ok && text.contains("与当前输出不一致"), "{text}");
        assert!(
            text.contains("UPDATE_SNAPSHOT=1 cargo test -p gate-demo"),
            "{text}"
        );
        assert_eq!(t.read("demo/probe"), stored);
        // UPDATE_SNAPSHOT=0 / 空串不算更新
        for off in ["0", ""] {
            let (ok, text) = probe(None, Some(off));
            assert!(!ok && text.contains("与当前输出不一致"), "{off:?}: {text}");
        }
        // CI 里带 UPDATE_SNAPSHOT：直接 panic，文件不动
        let (ok, text) = probe(Some("true"), Some("1"));
        assert!(!ok && text.contains("CI 里永远不改写快照"), "{text}");
        assert_eq!(t.read("demo/probe"), stored);
        // 只有 UPDATE_SNAPSHOT：改写文件并打印统计
        let (ok, text) = probe(None, Some("1"));
        assert!(ok, "{text}");
        assert!(text.contains("改动 1 条、删除 0 条、共 1 条"), "{text}");
        assert_eq!(t.read("demo/probe"), "{\"in\":{\"n\":2},\"out\":4}\n");
        // CI 里只比较：通过
        let (ok, text) = probe(Some("true"), None);
        assert!(ok, "{text}");
    }

    #[test]
    fn second_update_changes_nothing() {
        let t = Tmp::new();
        // 起点故意写得不规范：有空白、键顺序不同、缺 out
        t.write(
            "demo/idem",
            &[
                r#"{ "in": {"n": 1, "k": "猫"}, "out": null }"#,
                r#"{"in":{"n":2}}"#,
            ],
        );
        let f = |c: &Case| Some(json!({"z": c.get("n")?.as_i64()?, "a": "猫", "f": 0.1 + 0.2}));
        let first = t.check("demo/idem", Mode::Update, f).unwrap().unwrap();
        assert_eq!(first.changed, 2);
        let after_first = t.read("demo/idem");
        let mtime = std::fs::metadata(t.file("demo/idem"))
            .unwrap()
            .modified()
            .unwrap();
        let second = t.check("demo/idem", Mode::Update, f).unwrap().unwrap();
        assert_eq!(
            second,
            Summary {
                changed: 0,
                deleted: 0,
                total: 2
            }
        );
        assert_eq!(t.read("demo/idem"), after_first);
        assert_eq!(
            std::fs::metadata(t.file("demo/idem"))
                .unwrap()
                .modified()
                .unwrap(),
            mtime,
            "内容没变就不该重写文件"
        );
        t.check("demo/idem", Mode::Check, f).unwrap();
    }

    #[test]
    fn file_format_is_compact_ordered_and_keeps_non_ascii() {
        let t = Tmp::new();
        t.write(
            "demo/fmt",
            &[r#"{"in":{"s":"猫头鹰 🦉","n":1},"out":null}"#],
        );
        t.check("demo/fmt", Mode::Update, |_| {
            Some(json!({
                "z": 1,
                "a": "猫\n\"\u{1}",
                "inf": crate::f(f64::INFINITY),
                "b": {"$b": "AAEC"},
                "map": {"$map": [[1, "x"]]},
                "f": [0.1 + 0.2, 1e16, 1.0, -0.0, 5e-324, 1e22],
                "big": [u64::MAX, i64::MIN],
            }))
        })
        .unwrap();
        assert_eq!(
            t.read("demo/fmt"),
            concat!(
                r#"{"in":{"s":"猫头鹰 🦉","n":1},"out":{"z":1,"a":"猫\n\"\u0001","inf":{"$f":"inf"},"#,
                r#""b":{"$b":"AAEC"},"map":{"$map":[[1,"x"]]},"#,
                r#""f":[0.30000000000000004,1e+16,1.0,-0.0,5e-324,1e+22],"#,
                r#""big":[18446744073709551615,-9223372036854775808]}}"#,
                "\n"
            )
        );
        // 读回来再比较：浮点逐位、键顺序都一致
        t.check("demo/fmt", Mode::Check, |_| {
            Some(json!({
                "z": 1,
                "a": "猫\n\"\u{1}",
                "inf": crate::f(f64::INFINITY),
                "b": {"$b": "AAEC"},
                "map": {"$map": [[1, "x"]]},
                "f": [0.1 + 0.2, 1e16, 1.0, -0.0, 5e-324, 1e22],
                "big": [u64::MAX, i64::MIN],
            }))
        })
        .unwrap();
    }

    #[test]
    fn suite_records_out_of_order_and_flags_missing_records() {
        let t = Tmp::new();
        t.write(
            "demo/suite",
            &[
                r#"{"in":{"n":1},"out":2}"#,
                r#"{"in":{"n":2},"out":4}"#,
                r#"{"in":{"n":3},"out":6}"#,
            ],
        );
        // 乱序记录
        let mut s = Suite::open_with(&t.manifest, "demo/suite", Mode::Check);
        let cases = s.cases();
        for i in [2, 0, 1] {
            s.record(i, double(&cases[i]));
        }
        assert_eq!(s.conclude(), Ok(None));
        // 漏记一条：默认模式算失败
        let mut s = Suite::open_with(&t.manifest, "demo/suite", Mode::Check);
        let cases = s.cases();
        for i in [0, 2] {
            s.record(i, double(&cases[i]));
        }
        let err = s.conclude().unwrap_err();
        assert!(
            err.contains("第 2 行") && err.contains("没有被执行"),
            "{err}"
        );
        // 更新模式下漏记的行原样保留
        let mut s = Suite::open_with(&t.manifest, "demo/suite", Mode::Update);
        s.record(0, Some(json!(100)));
        let summary = s.conclude().unwrap().unwrap();
        assert_eq!(
            summary,
            Summary {
                changed: 1,
                deleted: 0,
                total: 3
            }
        );
        assert_eq!(
            t.read("demo/suite"),
            "{\"in\":{\"n\":1},\"out\":100}\n{\"in\":{\"n\":2},\"out\":4}\n{\"in\":{\"n\":3},\"out\":6}\n"
        );
    }

    #[test]
    #[should_panic(expected = "被记录了两次")]
    fn suite_rejects_recording_twice() {
        let t = Tmp::new();
        t.write("demo/twice", &[r#"{"in":{},"out":1}"#]);
        let mut s = Suite::open_with(&t.manifest, "demo/twice", Mode::Check);
        s.record(0, Some(json!(1)));
        s.record(0, Some(json!(1)));
    }

    #[test]
    #[should_panic(expected = "record(5) 越界")]
    fn suite_rejects_out_of_range_index() {
        let t = Tmp::new();
        t.write("demo/range", &[r#"{"in":{},"out":1}"#]);
        let mut s = Suite::open_with(&t.manifest, "demo/range", Mode::Check);
        s.record(5, Some(json!(1)));
    }

    #[test]
    #[should_panic(expected = "读不到快照")]
    fn missing_file_panics() {
        let t = Tmp::new();
        let _ = Suite::open_with(&t.manifest, "demo/absent", Mode::Check);
    }

    #[test]
    #[should_panic(expected = "一条用例都没有")]
    fn empty_file_panics() {
        let t = Tmp::new();
        t.write("demo/empty", &[""]);
        let _ = Suite::open_with(&t.manifest, "demo/empty", Mode::Check);
    }

    #[test]
    #[should_panic(expected = "多出键")]
    fn extra_keys_are_rejected() {
        let t = Tmp::new();
        t.write("demo/extra", &[r#"{"in":{},"out":1,"note":"x"}"#]);
        let _ = Suite::open_with(&t.manifest, "demo/extra", Mode::Check);
    }

    #[test]
    #[should_panic(expected = "缺少 \"in\"")]
    fn missing_in_is_rejected() {
        let t = Tmp::new();
        t.write("demo/noin", &[r#"{"out":1}"#]);
        let _ = Suite::open_with(&t.manifest, "demo/noin", Mode::Check);
    }

    #[test]
    #[should_panic(expected = "不是合法 JSON")]
    fn broken_json_is_rejected() {
        let t = Tmp::new();
        t.write("demo/broken", &[r#"{"in":"#]);
        let _ = Suite::open_with(&t.manifest, "demo/broken", Mode::Check);
    }

    #[test]
    fn blank_lines_are_skipped_but_line_numbers_stay_physical() {
        let t = Tmp::new();
        t.write(
            "demo/blank",
            &[r#"{"in":{"n":1},"out":2}"#, "", r#"{"in":{"n":2},"out":0}"#],
        );
        let err = t.check("demo/blank", Mode::Check, double).unwrap_err();
        assert!(err.contains("第 3 行"), "{err}");
        // 重写时空行被丢掉
        t.check("demo/blank", Mode::Update, double).unwrap();
        assert_eq!(
            t.read("demo/blank"),
            "{\"in\":{\"n\":1},\"out\":2}\n{\"in\":{\"n\":2},\"out\":4}\n"
        );
    }

    #[test]
    fn test_target_is_parsed_from_the_exe_name() {
        assert_eq!(
            test_target_from_stem("quota_snapshot-1a2b3c4d5e6f7a8b").as_deref(),
            Some("quota_snapshot")
        );
        assert_eq!(
            test_target_from_stem("admin-members-0123456789abcdef").as_deref(),
            Some("admin-members")
        );
        assert_eq!(test_target_from_stem("quota_snapshot"), None);
        assert_eq!(test_target_from_stem("a-nothex"), None);
        assert_eq!(test_target_from_stem("a-1a2b3c4d5e6f7a8bc"), None);
    }

    #[test]
    fn update_command_names_the_crate_and_the_update_flag() {
        let cmd = update_command("/work/repo/crates/gate-core");
        assert!(
            cmd.starts_with("UPDATE_SNAPSHOT=1 cargo test -p gate-core"),
            "{cmd}"
        );
    }
}
