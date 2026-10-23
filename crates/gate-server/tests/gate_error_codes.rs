//! 成员接口的每一个 4xx 拒绝都要在源头带原因码（`GateError::with_code`）：
//! 用量日志、单 Key 限流和统计都按码数，不能再靠写日志时按中文文案兜底匹配。
//!
//! 这里扫 `src/` 里所有 `GateError::new(...)`：状态码必须是 ≥ 500 的字面量，
//! 或者是紧跟 `.from_upstream(true)` 的上游状态透传（那是上游错误，记为 error，不是拒绝）。

use std::path::Path;

fn rs_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            rs_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// `s[open]` 是 `(`：返回配对的 `)` 之后的位置（跳过字符串里的括号）。
fn close_paren(s: &[u8], open: usize) -> usize {
    let (mut depth, mut i, mut in_str) = (0, open, false);
    while i < s.len() {
        match (in_str, s[i]) {
            (true, b'\\') => i += 1,
            (true, b'"') => in_str = false,
            (false, b'"') => in_str = true,
            (false, b'(') => depth += 1,
            (false, b')') => {
                depth -= 1;
                if depth == 0 {
                    return i + 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    panic!("unbalanced GateError::new(");
}

#[test]
fn every_4xx_gate_error_carries_a_reason_code() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rs_files(&src, &mut files);
    let tag = "GateError::new(";
    let mut bad = Vec::new();
    let mut seen = 0;
    for f in files {
        let text = std::fs::read_to_string(&f).unwrap();
        let bytes = text.as_bytes();
        let mut from = 0;
        while let Some(off) = text[from..].find(tag) {
            let start = from + off;
            let open = start + tag.len() - 1;
            let end = close_paren(bytes, open);
            from = end;
            seen += 1;
            let args = &text[open + 1..end - 1];
            let status = args.split(',').next().unwrap_or("").trim();
            let ok = match status.parse::<u16>() {
                Ok(n) => n >= 500,
                Err(_) => text[end..].trim_start().starts_with(".from_upstream(true)"),
            };
            if !ok {
                let line = text[..start].matches('\n').count() + 1;
                bad.push(format!(
                    "{}:{line}",
                    f.strip_prefix(&src).unwrap().display()
                ));
            }
        }
    }
    assert!(seen >= 5, "扫描没找到 GateError::new，路径不对？");
    assert!(
        bad.is_empty(),
        "这些 4xx 拒绝没有带原因码，改用 GateError::with_code(…, reasons::…)：{bad:?}"
    );
}
