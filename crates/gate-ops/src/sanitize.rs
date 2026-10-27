//!
//! 去掉：成员 API Key 原文（换成不可逆短哈希，同一把 Key 仍能对上）、盐 / 密码哈希等密钥类设置、
//! 请求词条哈希、报错堆栈、提示词 / 负面词 / 附加参数原文（成员创作内容）、缩略图、头像。
//! 保留：用量、请求特征哈希、网段、Discord ID 等分析需要的字段。原地改写，最后 VACUUM
//! （不 VACUUM 的话，被清掉的内容还留在空闲页里，用 strings 就能翻出来）。
//!
//! 相对旧版额外清掉的（旧版在这些表出现之前写的，漏了）：
//! - `key_donations.token`：捐赠者提交的 NovelAI Token 原文，同样换成短哈希；`contact` / `message` 清空；
//! - `discord_bans.avatar`：头像哈希；
//! - `discord_user_connections`：外部账号 ID 换成短哈希，显示名清空。

use anyhow::{Context, Result, bail};
use rusqlite::Connection;
use sha2::{Digest, Sha256};
use std::path::Path;
use std::time::Duration;

/// `"sha256:" + sha256(str(tok)).hexdigest()[:16]`
pub fn short_hash(token: &str) -> String {
    format!(
        "sha256:{}",
        &hex::encode(Sha256::digest(token.as_bytes()))[..16]
    )
}

/// 在 `path` 上原地脱敏，返回处理的 Key 数。任何一步失败都整体回滚、返回错误（调用方不要发布这份文件）。
pub fn sanitize(path: &Path) -> Result<usize> {
    let mut conn = Connection::open(path).with_context(|| format!("打不开 {}", path.display()))?;
    conn.busy_timeout(Duration::from_secs(15))?;
    let tx = conn.transaction()?;

    let rows: Vec<(i64, String)> = {
        let mut stmt = tx.prepare("SELECT id, token FROM api_keys")?;
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?
    };
    for (id, tok) in &rows {
        tx.execute(
            "UPDATE api_keys SET token=? WHERE id=?",
            rusqlite::params![short_hash(tok), id],
        )?;
    }
    // 名字里带密码 / 哈希 / 盐 / 密钥 / token 的设置一律删掉（10/11 审查：admin_password_hash 曾漏掉）
    tx.execute(
        "DELETE FROM site_settings WHERE key IN ('key_source_salt','admin_password_hash') OR key LIKE '%password%' \
         OR key LIKE '%hash%' OR key LIKE '%salt%' OR key LIKE '%secret%' OR key LIKE '%token%'",
        [],
    )?;
    // req_features（已随防分享体系移除，2026-10-13）的 toks 是提示词词条的无盐短哈希，能还原词条。
    // 新库不再有这张表；老快照里若还带着，整张删掉（比清空 toks 更干净，也不依赖表是否存在）。
    tx.execute("DROP TABLE IF EXISTS req_features", [])?;
    // Bug 追踪（error_events，10-27 删除）的详情可能夹带请求内容：老快照里若还带着，整张删掉。
    tx.execute("DROP TABLE IF EXISTS error_events", [])?;
    tx.execute(
        "UPDATE generation_audit SET prompt='', negative='', extra=''",
        [],
    )?;
    tx.execute("UPDATE discord_registrations SET avatar=''", [])?;

    // —— 以下是相对旧版新增的（见模块文档）——
    // 用量日志的来源 IP：3.0（10-18 起）存的是完整 IP，快照里只留前两段（a.b.*.*，和 2.x 存的一样），
    // 网段分析照样能做，但不能定位到个人。IPv6 只留前两组。已经打过码的老行（含 `*`）不动。
    tx.execute(
        "UPDATE usage_log SET src = CASE \
           WHEN instr(src, ':') > 0 THEN substr(src, 1, instr(substr(src, instr(src, ':') + 1), ':') + instr(src, ':')) || '*' \
           ELSE substr(src, 1, instr(src, '.') + instr(substr(src, instr(src, '.') + 1), '.')) || '*.*' END \
         WHERE src <> '' AND instr(src, '*') = 0",
        [],
    )?;
    let donated: Vec<(i64, String)> = {
        let mut stmt = tx.prepare("SELECT id, token FROM key_donations")?;
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?
    };
    for (id, tok) in &donated {
        tx.execute(
            "UPDATE key_donations SET token=?, contact='', message='' WHERE id=?",
            rusqlite::params![short_hash(tok), id],
        )?;
    }
    tx.execute("UPDATE discord_bans SET avatar=''", [])?;
    // 网页登录时读到的外部账号（10-23 起）：平台上的账号 ID 换成短哈希（跨 Discord 账号仍能对上），显示名清空。
    // 老快照里没有这张表就跳过。
    let has_conns: bool = tx.query_row(
        "SELECT COUNT(*)>0 FROM sqlite_master WHERE type='table' AND name='discord_user_connections'",
        [],
        |r| r.get(0),
    )?;
    if has_conns {
        let conns: Vec<(i64, String)> = {
            let mut stmt = tx.prepare(
                "SELECT rowid, ext_id FROM discord_user_connections WHERE ext_id NOT LIKE 'sha256:%'",
            )?;
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?
        };
        for (rowid, ext) in &conns {
            tx.execute(
                "UPDATE discord_user_connections SET ext_id=?, name='' WHERE rowid=?",
                rusqlite::params![short_hash(ext), rowid],
            )?;
        }
    }

    let left: i64 = tx.query_row(
        "SELECT COUNT(*) FROM api_keys WHERE token NOT LIKE 'sha256:%'",
        [],
        |r| r.get(0),
    )?;
    if left != 0 {
        bail!("api_keys.token 没脱敏干净（剩 {left} 条）");
    }
    tx.commit()?;
    conn.execute_batch("VACUUM")?;
    conn.close().map_err(|(_, e)| e)?;
    Ok(rows.len())
}
