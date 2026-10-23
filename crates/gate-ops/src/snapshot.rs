//! 一致快照（移植自旧版 snapshot.py，见标签 py-final，加上 naigate-snapshot 的「完整性校验 → gzip（权限 600）」两步）。
//!
//! 为什么不用 sqlite3 `.backup` / backup API：库里 90% 是审计原图 BLOB（曾达 1.3GB），整库复制 + VACUUM
//! 要好几分钟、磁盘 IO 打满，网关写库等锁超过 15 秒就报 database is locked（2026-10-10 两次部署，
//! 成员收到几十个 500）。现在：同一个读事务里把每张表 `INSERT ... SELECT` 到新库，
//! `generation_audit` 不读原图那几列（列在行尾，不选就不会读溢出页），只读几十 MB。WAL 下读事务不挡写。

use crate::util;
use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OpenFlags};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// 原图（BLOB 或文件）不进备份：缩略图（thumb）照常保留。
const SKIP: [(&str, &[&str]); 1] = [(
    "generation_audit",
    &["image", "image_type", "image_file", "image_size"],
)];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Info {
    /// `PRAGMA integrity_check` 的第一行；正常是 "ok"
    pub integrity: String,
    /// `api_keys` 行数（人工核对用）
    pub keys: i64,
}

fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// 把 `src` 的一致快照（不含原图）写到新库 `out`（`out` 必须不存在或为空）。
pub fn make_snapshot(src: &Path, out: &Path) -> Result<Info> {
    let dst = Connection::open_with_flags(
        out,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_URI,
    )
    .with_context(|| format!("创建 {} 失败", out.display()))?;
    dst.busy_timeout(Duration::from_secs(15))?;
    dst.execute(
        "ATTACH DATABASE ?1 AS s",
        [util::sqlite_uri(src, "mode=ro")],
    )
    .with_context(|| format!("以只读方式附加 {} 失败", src.display()))?;
    dst.execute_batch("BEGIN")?; // 一个读事务：所有表是同一时刻的数据

    let objs: Vec<(String, String, String)> = {
        let mut stmt = dst.prepare(
            "SELECT type, name, sql FROM s.sqlite_master WHERE sql IS NOT NULL AND name NOT LIKE 'sqlite_%' \
             ORDER BY CASE type WHEN 'table' THEN 0 ELSE 1 END",
        )?;
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?
    };
    for (typ, name, sql) in &objs {
        if typ != "table" {
            continue;
        }
        dst.execute_batch(sql)?;
        let cols: Vec<String> = {
            let mut stmt = dst.prepare(&format!("PRAGMA s.table_info({})", quote_ident(name)))?;
            stmt.query_map([], |r| r.get::<_, String>(1))?
                .collect::<rusqlite::Result<_>>()?
        };
        let skip: &[&str] = SKIP.iter().find(|(t, _)| t == name).map_or(&[], |(_, c)| c);
        let keep: Vec<String> = cols
            .iter()
            .filter(|c| !skip.contains(&c.as_str()))
            .map(|c| quote_ident(c))
            .collect();
        let cl = keep.join(", ");
        dst.execute_batch(&format!(
            "INSERT INTO main.{t} ({cl}) SELECT {cl} FROM s.{t}",
            t = quote_ident(name)
        ))?;
    }
    // 索引 / 触发器 / 视图放在灌完数据之后：灌数据时不触发触发器，也不用边插边维护索引
    for (typ, _, sql) in &objs {
        if matches!(typ.as_str(), "index" | "trigger" | "view") {
            dst.execute_batch(sql)?;
        }
    }
    let has_seq: bool = dst
        .query_row(
            "SELECT 1 FROM s.sqlite_master WHERE name='sqlite_sequence'",
            [],
            |_| Ok(()),
        )
        .is_ok();
    if has_seq {
        dst.execute_batch(
            "DELETE FROM main.sqlite_sequence; INSERT INTO main.sqlite_sequence SELECT * FROM s.sqlite_sequence;",
        )?;
    }
    dst.execute_batch("COMMIT")?;
    dst.execute_batch("DETACH DATABASE s")?;
    let integrity: String = dst.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
    let keys: i64 = dst.query_row("SELECT COUNT(*) FROM api_keys", [], |r| r.get(0))?;
    dst.close().map_err(|(_, e)| e)?;
    Ok(Info { integrity, keys })
}

/// 把 `from` gzip 到 `to`（先写临时文件再改名；文件权限 600，创建时就是 600，不留宽权限窗口）。
pub fn gzip_file(from: &Path, to: &Path) -> Result<u64> {
    let part = to.with_file_name(format!(
        ".{}.part",
        to.file_name()
            .map_or_else(|| "snapshot".into(), |n| n.to_string_lossy().into_owned())
    ));
    let result = (|| -> Result<u64> {
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
        let file = opts
            .open(&part)
            .with_context(|| format!("创建 {} 失败", part.display()))?;
        let mut enc = flate2::write::GzEncoder::new(
            std::io::BufWriter::new(file),
            flate2::Compression::default(),
        );
        let mut src = std::fs::File::open(from)?;
        std::io::copy(&mut src, &mut enc)?;
        enc.finish()?.flush()?;
        std::fs::rename(&part, to).with_context(|| format!("改名到 {} 失败", to.display()))?;
        Ok(std::fs::metadata(to)?.len())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    result
}

/// 退出时清理临时快照（无论成功失败）。
struct TempFile(PathBuf);
impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
        // 回滚日志 / WAL 残留
        for suffix in ["-journal", "-wal", "-shm"] {
            let mut p = self.0.clone().into_os_string();
            p.push(suffix);
            let _ = std::fs::remove_file(PathBuf::from(p));
        }
    }
}

/// `nai-ops snapshot <out.db.gz>`：快照 → 完整性校验 → gzip（600）。返回要打印的那一行。
pub fn run(src: &Path, out: &Path) -> Result<String> {
    let dir = out
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let tmp = TempFile(dir.join(format!(
        ".snap-{}-{}.db",
        std::process::id(),
        util::now_secs() as u64
    )));
    let info = make_snapshot(src, &tmp.0)?;
    if info.integrity != "ok" {
        bail!(
            "snapshot FAILED integrity_check: {} {}",
            info.integrity,
            info.keys
        );
    }
    let size = gzip_file(&tmp.0, out)?;
    Ok(format!(
        "snapshot {} ({}, integrity ok, keys={})",
        out.display(),
        util::human_size(size),
        info.keys
    ))
}
