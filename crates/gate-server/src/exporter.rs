//! 把成员的原图打包成 ZIP。只有后台「导出该成员全部」用；
//! 成员自助导出（含 EPUB 电子书）已在 3.0 删除（站长决定不再提供）。
//!
//! 行是 `db.audit_images_for_key` 的结果。可能较大较慢，调用方放到阻塞线程里跑。

use gate_core::clock::Clock;
use gate_core::pyext::py_str;
use gate_core::pyval::truthy;
use gate_db::repo::audit::{AuditImage, AuditRecord};
use serde_json::Value;
use std::io::{Cursor, Write};
use zip::CompressionMethod;
use zip::write::SimpleFileOptions;

/// 原图扩展名（其余类型落 `.png`）。
fn ext_of(image_type: &str) -> &'static str {
    match image_type {
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        _ => "png",
    }
}

/// prompts.txt 里的一条记录：`head` 行 + 正面 / 负面 + 可选的「参数：」行 + 末尾空行。
fn prompt_entry(head: &str, prompt: &str, negative: &str, extra: &str) -> String {
    let params = params_line(extra);
    let mut entry = format!("{head}\n正面：{prompt}\n负面：{negative}");
    if !params.is_empty() {
        entry.push_str(&format!("\n参数：{params}"));
    }
    entry.push('\n');
    entry
}

/// 一行参数摘要：`种子 42 · k_euler · 28步 · CFG 5 · 832×1216`。额外信息读不出来就是空串。
fn params_line(extra: &str) -> String {
    let Ok(v) = serde_json::from_str::<Value>(extra) else {
        return String::new();
    };
    let p = v.get("params").filter(|p| truthy(p));
    let Some(p) = p.and_then(Value::as_object) else {
        return String::new();
    };
    let get = |k: &str| p.get(k).unwrap_or(&Value::Null);
    let mut bits = Vec::new();
    if !get("seed").is_null() {
        bits.push(format!("种子 {}", py_str(get("seed"))));
    }
    if truthy(get("sampler")) {
        bits.push(py_str(get("sampler")));
    }
    if truthy(get("steps")) {
        bits.push(format!("{}步", py_str(get("steps"))));
    }
    if !get("scale").is_null() {
        bits.push(format!("CFG {}", py_str(get("scale"))));
    }
    if truthy(get("width")) && truthy(get("height")) {
        bits.push(format!(
            "{}×{}",
            py_str(get("width")),
            py_str(get("height"))
        ));
    }
    bits.join(" · ")
}

/// zip 条目的修改时间（本地时间；超出 zip 能表示的范围就用默认值）。
fn zip_time(clock: &Clock, ts: f64) -> Option<zip::DateTime> {
    let z = jiff::Timestamp::from_second(ts as i64)
        .ok()?
        .to_zoned(clock.tz().clone());
    zip::DateTime::from_date_and_time(
        u16::try_from(z.year()).ok()?,
        u8::try_from(z.month()).ok()?,
        u8::try_from(z.day()).ok()?,
        u8::try_from(z.hour()).ok()?,
        u8::try_from(z.minute()).ok()?,
        u8::try_from(z.second()).ok()?,
    )
    .ok()
}

/// 只存不压（图片本身已压缩），修改时间取导出时刻。
fn stored_opts(clock: &Clock, now: f64) -> SimpleFileOptions {
    let o = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    match zip_time(clock, now) {
        Some(t) => o.last_modified_time(t),
        None => o,
    }
}

/// 原图 zip + prompts.txt（每张的正面 / 负面 / 参数）。图片本身已压缩，所以只存不压（`ZIP_STORED`）。
pub fn build_zip(rows: &[AuditImage], who: &str, clock: &Clock, now: f64) -> Vec<u8> {
    let mut buf = Cursor::new(Vec::new());
    let mut lines = vec![
        format!(
            "{who} 的作品 · 共 {} 张 · 导出于 {}",
            rows.len(),
            clock.format(now, "%Y-%m-%d %H:%M")
        ),
        String::new(),
    ];
    {
        let mut z = zip::ZipWriter::new(&mut buf);
        let stored = stored_opts(clock, now);
        for (i, r) in rows.iter().enumerate() {
            let stamp = clock.format(r.ts, "%Y%m%d-%H%M%S");
            let fname = format!("{:04}_{stamp}.{}", i + 1, ext_of(&r.image_type));
            if z.start_file(&fname, stored).is_ok() {
                let _ = z.write_all(&r.image);
            }
            lines.push(prompt_entry(
                &format!("[{fname}]"),
                &r.prompt,
                &r.negative,
                &r.extra,
            ));
        }
        if z.start_file("prompts.txt", stored).is_ok() {
            let _ = z.write_all(lines.join("\n").as_bytes());
        }
        let _ = z.finish();
    }
    buf.into_inner()
}

/// zip 子文件夹名：去掉斜杠 / 反斜杠 / 控制字符和前后点，空了用占位，避免 zip-slip 与非法路径。
fn zip_folder(label: &str) -> String {
    let cleaned: String = label
        .chars()
        .map(|c| {
            if c == '/' || c == '\\' || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    let trimmed = cleaned.trim().trim_matches('.').trim();
    if trimmed.is_empty() {
        "成员".to_string()
    } else {
        trimmed.to_string()
    }
}

/// 多人导出里的一个账号：全部生成记录（文字）+ 还留着的原图。
pub struct ExportGroup {
    /// 文件夹名（调用方保证带 Key 编号、互不相同）
    pub label: String,
    pub records: Vec<AuditRecord>,
    pub images: Vec<AuditImage>,
    /// 原图超出这次导出的总上限、没打包进来
    pub images_truncated: bool,
}

/// 多个成员合并成一个 zip：每个账号一个文件夹，里面是原图和这个账号自己的 prompts.txt；
/// 原图已过期的记录照样列出提示词（标「原图已过期」），一张图都没有的账号也有文件夹。
/// 根目录的「说明.txt」列出每个账号的记录数和原图数。用于一次导出多个疑似小号做人工复核。
pub fn build_zip_groups(groups: &[ExportGroup], clock: &Clock, now: f64) -> Vec<u8> {
    use std::collections::HashMap;
    let mut buf = Cursor::new(Vec::new());
    let mut summary = vec![
        format!(
            "选中 {} 个账号 · 导出于 {}",
            groups.len(),
            clock.format(now, "%Y-%m-%d %H:%M")
        ),
        String::new(),
    ];
    {
        let mut z = zip::ZipWriter::new(&mut buf);
        let stored = stored_opts(clock, now);
        for g in groups {
            let folder = zip_folder(&g.label);
            let _ = z.add_directory(format!("{folder}/"), stored);
            let by_id: HashMap<i64, &AuditImage> = g.images.iter().map(|i| (i.id, i)).collect();
            let mut lines = vec![
                format!(
                    "{} · 记录 {} 条 · 原图 {} 张",
                    g.label,
                    g.records.len(),
                    by_id.len()
                ),
                String::new(),
            ];
            let mut n = 0;
            for r in &g.records {
                let stamp = clock.format(r.ts, "%Y%m%d-%H%M%S");
                let head = match by_id.get(&r.id) {
                    Some(img) => {
                        n += 1;
                        let fname = format!("{n:04}_{stamp}.{}", ext_of(&img.image_type));
                        if z.start_file(format!("{folder}/{fname}"), stored).is_ok() {
                            let _ = z.write_all(&img.image);
                        }
                        format!("[{fname}]")
                    }
                    None if r.has_image && g.images_truncated => {
                        format!("[{stamp} · 原图超出本次导出上限，未打包]")
                    }
                    None => format!("[{stamp} · 原图已过期]"),
                };
                lines.push(prompt_entry(&head, &r.prompt, &r.negative, &r.extra));
            }
            if z.start_file(format!("{folder}/prompts.txt"), stored)
                .is_ok()
            {
                let _ = z.write_all(lines.join("\n").as_bytes());
            }
            summary.push(format!(
                "{folder}/ · 记录 {} 条 · 原图 {} 张{}",
                g.records.len(),
                n,
                if g.images_truncated {
                    "（原图超出上限，部分未打包）"
                } else {
                    ""
                }
            ));
        }
        if z.start_file("说明.txt", stored).is_ok() {
            let _ = z.write_all(summary.join("\n").as_bytes());
        }
        let _ = z.finish();
    }
    buf.into_inner()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    const PNG: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 0x0d, 0x49, 0x48, 0x44, 0x52, 0,
        0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0, 0x1f, 0x15, 0xc4, 0x89,
    ];

    fn rows() -> Vec<AuditImage> {
        vec![
            AuditImage {
                id: 1,
                ts: 1_000_000.0,
                image_type: "image/png".into(),
                prompt: "1girl, forest".into(),
                negative: "lowres".into(),
                extra: r#"{"params":{"seed":42,"sampler":"k_euler","steps":28,"scale":5,"width":832,"height":1216}}"#.into(),
                image: PNG.to_vec(),
            },
            AuditImage {
                id: 2,
                ts: 1_000_100.0,
                image_type: "image/png".into(),
                prompt: "1boy, city".into(),
                negative: String::new(),
                extra: String::new(),
                image: PNG.to_vec(),
            },
        ]
    }

    #[test]
    fn zip_has_images_and_prompts() {
        let clock = Clock::new("Asia/Shanghai", 7);
        let data = build_zip(&rows(), "小明", &clock, 1_760_000_000.0);
        let mut z = zip::ZipArchive::new(Cursor::new(data)).unwrap();
        let names: Vec<String> = z
            .file_names()
            .filter_map(Result::ok)
            .map(std::borrow::Cow::into_owned)
            .collect();
        assert_eq!(names.iter().filter(|n| n.ends_with(".png")).count(), 2);
        let mut txt = String::new();
        z.by_name("prompts.txt")
            .unwrap()
            .read_to_string(&mut txt)
            .unwrap();
        assert!(txt.starts_with("小明 的作品 · 共 2 张 · 导出于 2025-10-09 16:53\n"));
        assert!(txt.contains(
            "正面：1girl, forest\n负面：lowres\n参数：种子 42 · k_euler · 28步 · CFG 5 · 832×1216\n"
        ));
        assert!(txt.contains("[0002_19700112-214820.png]\n正面：1boy, city\n负面：\n"));
    }

    #[test]
    fn groups_zip_has_per_member_folders() {
        let clock = Clock::new("Asia/Shanghai", 7);
        let rec = |i: &AuditImage, has_image: bool| AuditRecord {
            id: i.id,
            ts: i.ts,
            prompt: i.prompt.clone(),
            negative: i.negative.clone(),
            extra: i.extra.clone(),
            has_image,
        };
        let imgs = rows();
        let groups = vec![
            ExportGroup {
                label: "紫霞-489".into(),
                records: imgs.iter().map(|i| rec(i, true)).collect(),
                images: rows(),
                images_truncated: false,
            },
            // 原图全过期：只有记录，照样有文件夹和提示词
            ExportGroup {
                label: "噜噜-253".into(),
                records: vec![rec(&imgs[0], false)],
                images: vec![],
                images_truncated: false,
            },
        ];
        let data = build_zip_groups(&groups, &clock, 1_760_000_000.0);
        let mut z = zip::ZipArchive::new(Cursor::new(data)).unwrap();
        let names: Vec<String> = z
            .file_names()
            .filter_map(Result::ok)
            .map(std::borrow::Cow::into_owned)
            .collect();
        assert!(
            names
                .iter()
                .any(|n| n == "紫霞-489/0001_19700112-214640.png")
        );
        assert!(
            names
                .iter()
                .any(|n| n == "紫霞-489/0002_19700112-214820.png")
        );
        assert!(names.iter().any(|n| n == "噜噜-253/"));
        assert!(names.iter().any(|n| n == "噜噜-253/prompts.txt"));
        assert_eq!(names.iter().filter(|n| n.ends_with(".png")).count(), 2);
        let mut txt = String::new();
        z.by_name("噜噜-253/prompts.txt")
            .unwrap()
            .read_to_string(&mut txt)
            .unwrap();
        assert!(txt.starts_with("噜噜-253 · 记录 1 条 · 原图 0 张\n"));
        assert!(txt.contains("[19700112-214640 · 原图已过期]\n正面：1girl, forest"));
        let mut sum = String::new();
        z.by_name("说明.txt")
            .unwrap()
            .read_to_string(&mut sum)
            .unwrap();
        assert!(sum.starts_with("选中 2 个账号 · 导出于 2025-10-09 16:53\n"));
        assert!(sum.contains("紫霞-489/ · 记录 2 条 · 原图 2 张"));
        assert!(sum.contains("噜噜-253/ · 记录 1 条 · 原图 0 张"));
    }

    #[test]
    fn folder_name_is_sanitized() {
        assert_eq!(zip_folder("a/b\\c"), "a_b_c");
        assert_eq!(zip_folder("  ..hack.. "), "hack");
        assert_eq!(zip_folder("   "), "成员");
    }
}
