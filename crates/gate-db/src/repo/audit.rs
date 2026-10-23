//! 生成审计（提示词 / 参数 / 缩略图 / 原图）。
//!
//! 原图写成文件（`<库目录>/audit_images/<YYYYMMDD>/<uuid>.<ext>`），库里只记相对路径和字节数；
//! 10-11 之前的老记录原图仍在 `image` BLOB 里，两处都能读。

use super::{HAS_IMAGE, IMAGE_SIZE, SqlValue, marks};
use crate::{Db, Result, value};
use gate_core::{clock::Clock, pyc};
use rusqlite::{OptionalExtension, params};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// `add_audit` 的入参。
#[derive(Debug, Clone, Copy, Default)]
pub struct NewAudit<'a> {
    pub key_id: Option<i64>,
    pub key_name: &'a str,
    pub kind: &'a str,
    pub model: &'a str,
    pub status: &'a str,
    pub prompt: &'a str,
    pub negative: &'a str,
    pub thumb: Option<&'a [u8]>,
    pub extra: &'a str,
    /// 原图字节；空 / None 表示不存原图。
    pub image: Option<&'a [u8]>,
    pub image_type: &'a str,
    /// 请求编号（与 usage_log.rid 相同）。
    pub rid: &'a str,
}

/// `audit_images_for_key` 产出的一条：(id, ts, image_type, prompt, negative, extra, 原图字节)。
#[derive(Debug, Clone, PartialEq)]
pub struct AuditImage {
    pub id: i64,
    pub ts: f64,
    pub image_type: String,
    pub prompt: String,
    pub negative: String,
    pub extra: String,
    pub image: Vec<u8>,
}

/// 导出时的一条生成记录（只有文字）。
#[derive(Debug, Clone)]
pub struct AuditRecord {
    pub id: i64,
    pub ts: f64,
    pub prompt: String,
    pub negative: String,
    pub extra: String,
    pub has_image: bool,
}

/// 导出查询的原始行：(id, ts, image_type, prompt, negative, extra, image BLOB, image_file)。
type RawAuditRow = (
    i64,
    f64,
    String,
    String,
    String,
    String,
    Option<Vec<u8>>,
    Option<String>,
);

/// 原图文件扩展名（旧版 `_EXT`，其余类型落 `.bin`）。
fn image_ext(image_type: &str) -> &'static str {
    match image_type {
        "image/png" => "png",
        "image/webp" => "webp",
        "image/jpeg" => "jpg",
        _ => "bin",
    }
}

/// `Path.resolve()`（非 strict）：存在的部分解析符号链接，不存在的尾部按字面拼接（`..` / `.` 做词法处理）。
fn resolve(p: &Path) -> PathBuf {
    if let Ok(c) = std::fs::canonicalize(p) {
        return c;
    }
    let mut missing: Vec<std::ffi::OsString> = Vec::new();
    let mut base = p.to_path_buf();
    while !base.exists() {
        match (base.file_name(), base.parent()) {
            (Some(name), Some(parent)) => {
                missing.push(name.to_os_string());
                base = parent.to_path_buf();
            }
            _ => break,
        }
    }
    let mut out = std::fs::canonicalize(&base).unwrap_or(base);
    for name in missing.into_iter().rev() {
        if name == ".." {
            out.pop();
        } else if name != "." {
            out.push(name);
        }
    }
    out
}

/// `root in f.parents`：f 必须真的落在 root 之下。
fn is_under(root: &Path, f: &Path) -> bool {
    f.ancestors().skip(1).any(|a| a == root)
}

fn write_image(
    dir: &Path,
    data: &[u8],
    image_type: &str,
    sub: &str,
    uuid_hex: &str,
) -> std::io::Result<String> {
    let folder = dir.join(sub);
    std::fs::create_dir_all(&folder)?;
    let rel = format!("{sub}/{uuid_hex}.{}", image_ext(image_type));
    let tmp = dir.join(format!("{rel}.tmp"));
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, dir.join(&rel))?;
    Ok(rel)
}

/// 只读本目录下的文件；越界 / 不存在 / 读失败都是 None。
fn read_image(dir: &Path, rel: &str) -> Option<Vec<u8>> {
    let f = resolve(&dir.join(rel));
    if !is_under(&resolve(dir), &f) {
        return None;
    }
    std::fs::read(f).ok()
}

fn unlink_images(dir: &Path, rels: &[String]) {
    let root = resolve(dir);
    for rel in rels {
        let f = resolve(&dir.join(rel));
        if is_under(&root, &f) {
            let _ = std::fs::remove_file(f);
        }
    }
}

/// 新生成一个 uuid4 风格的 32 位十六进制串（旧版 `uuid4().hex`）。
fn uuid_hex() -> String {
    let mut n: u128 = rand::random();
    n = (n & !(0xf_u128 << 76)) | (4_u128 << 76); // version 4
    n = (n & !(0x3_u128 << 62)) | (2_u128 << 62); // variant 10
    format!("{n:032x}")
}

/// 阻塞的文件任务放到 blocking 线程池（旧版 `asyncio.to_thread`）。
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|_| crate::DbError::ReaderPanicked)
}

impl Db {
    /// 记一条审计。原图先写文件（再入库）；磁盘满 / 目录不可写时只记提示词和参数，不存原图。
    /// 入库失败时把刚写的文件删掉再报错。`now` 同时决定库里的 ts 和原图所在日期目录（本地日期）。
    pub async fn add_audit(&self, a: &NewAudit<'_>, now: f64, clock: &Clock) -> Result<()> {
        self.add_audit_with_id(a, now, clock, &uuid_hex()).await
    }

    /// 同 [`add_audit`](Self::add_audit)，文件名里的 uuid 由调用方给（对拍测试用）。
    pub async fn add_audit_with_id(
        &self,
        a: &NewAudit<'_>,
        now: f64,
        clock: &Clock,
        uuid_hex: &str,
    ) -> Result<()> {
        let mut image = a.image.filter(|b| !b.is_empty());
        let dir = self.image_dir().to_path_buf();
        let mut rel: Option<String> = None;
        if let Some(data) = image {
            let (data, image_type) = (data.to_vec(), a.image_type.to_string());
            let sub = clock.format(now, "%Y%m%d");
            let uuid = uuid_hex.to_string();
            let d = dir.clone();
            match blocking(move || write_image(&d, &data, &image_type, &sub, &uuid)).await? {
                Ok(r) => rel = Some(r),
                Err(e) => {
                    // 磁盘满 / 目录不可写：原图不存，提示词和参数照常记录
                    tracing::warn!("[audit] 原图写文件失败，只记录提示词：{e}");
                    image = None;
                }
            }
        }
        let (key_id, now_) = (a.key_id, now);
        let key_name = pyc::take_chars(a.key_name, 80).to_string();
        let kind = a.kind.to_string();
        let model = pyc::take_chars(a.model, 80).to_string();
        let status = a.status.to_string();
        let prompt = a.prompt.to_string();
        let negative = a.negative.to_string();
        let extra = a.extra.to_string();
        let rid = a.rid.to_string();
        let thumb = a.thumb.map(<[u8]>::to_vec);
        let image_type = if image.is_some() {
            a.image_type.to_string()
        } else {
            String::new()
        };
        let size = image.map(|b| b.len() as i64);
        let rel_ = rel.clone();
        let res = self
            .write(move |tx| {
                tx.execute(
                    "INSERT INTO generation_audit (ts,key_id,key_name,kind,model,status,prompt,negative,extra,thumb, \
                                                   image,image_type,image_file,image_size,rid) \
                     VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
                    params![
                        now_,
                        key_id,
                        key_name,
                        kind,
                        model,
                        status,
                        prompt,
                        negative,
                        extra,
                        thumb,
                        Option::<Vec<u8>>::None,
                        image_type,
                        rel_,
                        size,
                        rid
                    ],
                )?;
                Ok(())
            })
            .await;
        if res.is_err()
            && let Some(r) = rel
        {
            let _ = blocking(move || unlink_images(&dir, &[r])).await;
        }
        res
    }

    /// 生成记录分页，按多把 Key（同一成员重置 / 换 Key 后的历史）一起查；`key_ids` 为空表示不筛。
    pub async fn list_audit_ids(
        &self,
        limit: i64,
        offset: i64,
        key_ids: &[i64],
    ) -> Result<(Vec<Value>, i64)> {
        let key_ids = key_ids.to_vec();
        self.read(move |c| {
            let where_ = if key_ids.is_empty() {
                String::new()
            } else {
                format!("WHERE key_id IN ({})", marks(key_ids.len()))
            };
            let total: i64 = c.query_row(
                &format!("SELECT COUNT(*) FROM generation_audit {where_}"),
                rusqlite::params_from_iter(key_ids.iter()),
                |r| value::i64_or(r, 0, 0),
            )?;
            let mut stmt = c.prepare(&format!(
                "SELECT id,ts,rid,key_id,key_name,kind,model,status,prompt,negative,extra, \
                        (thumb IS NOT NULL) AS has_thumb, (image IS NOT NULL OR image_file IS NOT NULL) AS has_image \
                 FROM generation_audit {where_} ORDER BY id DESC LIMIT ? OFFSET ?"
            ))?;
            let mut args: Vec<SqlValue> = key_ids.iter().map(|k| SqlValue::Integer(*k)).collect();
            args.push(SqlValue::Integer(limit));
            args.push(SqlValue::Integer(offset));
            let rows = stmt
                .query_map(rusqlite::params_from_iter(args.iter()), value::row_json)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok((rows, total))
        })
        .await
    }

    pub async fn audit_thumb(&self, audit_id: i64) -> Result<Option<Vec<u8>>> {
        self.read(move |c| {
            let thumb: Option<Option<Vec<u8>>> = c
                .query_row(
                    "SELECT thumb FROM generation_audit WHERE id=?",
                    [audit_id],
                    |r| r.get::<_, Option<Vec<u8>>>(0),
                )
                .optional()?;
            Ok(thumb.flatten())
        })
        .await
    }

    /// 取原图字节与类型：先看 BLOB（老记录），再读文件；都没有 / 文件丢了返回 (None, "")。
    pub async fn audit_image(&self, audit_id: i64) -> Result<(Option<Vec<u8>>, String)> {
        type Raw = (Option<Vec<u8>>, Option<String>, Option<String>);
        let raw: Option<Raw> = self
            .read(move |c| {
                c.query_row(
                    "SELECT image, image_file, image_type FROM generation_audit WHERE id=?",
                    [audit_id],
                    |r| {
                        Ok((
                            r.get::<_, Option<Vec<u8>>>(0)?,
                            value::text_opt(r, 1)?,
                            value::text_opt(r, 2)?,
                        ))
                    },
                )
                .optional()
            })
            .await?;
        let Some((image, image_file, image_type)) = raw else {
            return Ok((None, String::new()));
        };
        let data = match image {
            Some(b) => Some(b),
            None => match image_file.filter(|f| !f.is_empty()) {
                Some(f) => {
                    let dir = self.image_dir().to_path_buf();
                    blocking(move || read_image(&dir, &f)).await?
                }
                None => None,
            },
        };
        Ok(match data {
            // `row["image_type"] or "image/png"`
            Some(d) => (
                Some(d),
                image_type
                    .filter(|t| !t.is_empty())
                    .unwrap_or_else(|| "image/png".into()),
            ),
            None => (None, String::new()),
        })
    }

    /// 某把 Key 的全部生成记录（只要文字，不读图片）：多人导出时，原图过期的账号也要把提示词列出来。
    /// 按时间先后排；`has_image` 表示原图还在不在。
    pub async fn audit_records_for_key(&self, key_id: i64) -> Result<Vec<AuditRecord>> {
        self.read(move |c| {
            c.prepare(&format!(
                "SELECT id, ts, prompt, negative, extra, {HAS_IMAGE} FROM generation_audit \
                 WHERE key_id=? ORDER BY ts"
            ))?
            .query_map([key_id], |r| {
                Ok(AuditRecord {
                    id: value::i64_or(r, 0, 0)?,
                    ts: value::f64_or(r, 1, 0.0)?,
                    prompt: value::text_or(r, 2, "")?,
                    negative: value::text_or(r, 3, "")?,
                    extra: value::text_or(r, 4, "")?,
                    has_image: value::i64_or(r, 5, 0)? != 0,
                })
            })?
            .collect()
        })
        .await
    }

    /// 后台导出用：某把 Key 带原图的记录。`max_bytes > 0` 时只取最新的、原图合计不超过这么多字节的那些
    /// （至少给一条），防止一次打包把内存撑爆。文件丢了的记录跳过。
    pub async fn audit_images_for_key(
        &self,
        key_id: i64,
        max_bytes: i64,
        since: f64,
    ) -> Result<Vec<AuditImage>> {
        let cols = "SELECT id, ts, image_type, prompt, negative, extra, image, image_file FROM generation_audit ";
        let raw: Vec<RawAuditRow> = self
            .read(move |c| {
                let map = |r: &rusqlite::Row<'_>| -> rusqlite::Result<RawAuditRow> {
                    Ok((
                        value::i64_or(r, 0, 0)?,
                        value::f64_or(r, 1, 0.0)?,
                        value::text_or(r, 2, "None")?,
                        value::text_or(r, 3, "None")?,
                        value::text_or(r, 4, "None")?,
                        value::text_or(r, 5, "None")?,
                        r.get::<_, Option<Vec<u8>>>(6)?,
                        value::text_opt(r, 7)?,
                    ))
                };
                if max_bytes > 0 {
                    let mut stmt = c.prepare(&format!(
                        "SELECT id, {IMAGE_SIZE} FROM generation_audit WHERE key_id=? AND {HAS_IMAGE} AND ts>=? \
                         ORDER BY ts DESC"
                    ))?;
                    let sizes = stmt
                        .query_map(params![key_id, since], |r| {
                            Ok((value::i64_or(r, 0, 0)?, value::i64_or(r, 1, 0)?))
                        })?
                        .collect::<rusqlite::Result<Vec<_>>>()?;
                    let (mut keep, mut total) = (Vec::new(), 0_i64);
                    for (rid, size) in sizes {
                        if total + size > max_bytes && !keep.is_empty() {
                            break;
                        }
                        keep.push(rid);
                        total += size;
                    }
                    if keep.is_empty() {
                        return Ok(Vec::new());
                    }
                    let mut stmt = c.prepare(&format!(
                        "{cols}WHERE id IN ({}) ORDER BY ts",
                        marks(keep.len())
                    ))?;
                    stmt.query_map(rusqlite::params_from_iter(keep.iter()), map)?
                        .collect()
                } else {
                    let mut stmt = c.prepare(&format!(
                        "{cols}WHERE key_id=? AND {HAS_IMAGE} AND ts>=? ORDER BY ts"
                    ))?;
                    stmt.query_map(params![key_id, since], map)?.collect()
                }
            })
            .await?;
        self.with_image(raw).await
    }

    /// 旧版 `_with_image`：把 BLOB / 文件里的原图字节取出来，文件丢了的跳过。
    async fn with_image(&self, rows: Vec<RawAuditRow>) -> Result<Vec<AuditImage>> {
        let dir = self.image_dir().to_path_buf();
        blocking(move || {
            let mut out = Vec::new();
            for (id, ts, image_type, prompt, negative, extra, blob, file) in rows {
                let data = match blob {
                    Some(b) => Some(b),
                    None => file.as_deref().and_then(|f| read_image(&dir, f)),
                };
                if let Some(image) = data {
                    out.push(AuditImage {
                        id,
                        ts,
                        image_type,
                        prompt,
                        negative,
                        extra,
                        image,
                    });
                }
            }
            out
        })
        .await
    }

    /// 原图总字节数（文件 + 老记录的 BLOB）。
    pub async fn audit_image_bytes(&self) -> Result<i64> {
        self.read(|c| {
            c.query_row(
                &format!(
                    "SELECT COALESCE(SUM({IMAGE_SIZE}), 0) FROM generation_audit WHERE {HAS_IMAGE}"
                ),
                [],
                |r| value::i64_or(r, 0, 0),
            )
        })
        .await
    }

    /// 原图总量超过 `cap_bytes` 时，从最旧的开始清，清到 cap 的 `low_water`（默认 0.9）以下
    /// （留余量，避免每 5 分钟清一点）。返回清掉的条数。
    pub async fn purge_audit_images_to_cap(&self, cap_bytes: i64, low_water: f64) -> Result<i64> {
        let total = self.audit_image_bytes().await?;
        if cap_bytes <= 0 || total <= cap_bytes {
            return Ok(0);
        }
        // Python: int(cap_bytes * low_water)——向零截断
        let target = total - (cap_bytes as f64 * low_water) as i64;
        let sizes: Vec<(i64, i64)> = self
            .read(|c| {
                let mut stmt = c.prepare(&format!(
                    "SELECT id, {IMAGE_SIZE} FROM generation_audit WHERE {HAS_IMAGE} ORDER BY id"
                ))?;
                stmt.query_map([], |r| {
                    Ok((value::i64_or(r, 0, 0)?, value::i64_or(r, 1, 0)?))
                })?
                .collect()
            })
            .await?;
        let (mut freed, mut cutoff) = (0_i64, None);
        for (row_id, size) in sizes {
            freed += size;
            cutoff = Some(row_id);
            if freed >= target {
                break;
            }
        }
        match cutoff {
            None => Ok(0),
            Some(c) => self.purge_audit_images_before_id(c + 1).await,
        }
    }

    pub async fn purge_audit_images_before_id(&self, before_id: i64) -> Result<i64> {
        self.drop_images("id<?", SqlValue::Integer(before_id)).await
    }

    /// 只清原图（文件 / BLOB），保留缩略图、提示词、参数。
    pub async fn purge_audit_images(&self, older_than: f64) -> Result<i64> {
        self.drop_images("ts<?", SqlValue::Real(older_than)).await
    }

    /// 清审计：先删原图文件，再删记录。返回删掉的记录数。
    pub async fn purge_audit(&self, older_than: f64) -> Result<i64> {
        self.drop_images("ts<?", SqlValue::Real(older_than)).await?;
        self.write(move |tx| {
            Ok(tx.execute("DELETE FROM generation_audit WHERE ts<?", [older_than])? as i64)
        })
        .await
    }

    /// 把满足 where 的记录的原图清掉：BLOB 置空、文件删除。返回清掉的条数。
    /// 先选出要清的记录，再只按这些 id 清（中间插进来的新记录不会被清了库、漏删文件）；
    /// 每 50 条一个事务，让别的请求能插进来（旧版 10-11 事故：一次改几 GB 把唯一连接占住十几分钟）。
    async fn drop_images(&self, where_: &'static str, arg: SqlValue) -> Result<i64> {
        let rows: Vec<(i64, Option<String>)> = {
            let arg = arg.clone();
            self.read(move |c| {
                let mut stmt = c.prepare(&format!(
                    "SELECT id, image_file FROM generation_audit WHERE {HAS_IMAGE} AND {where_}"
                ))?;
                stmt.query_map([arg], |r| {
                    Ok((value::i64_or(r, 0, 0)?, value::text_opt(r, 1)?))
                })?
                .collect()
            })
            .await?
        };
        let ids: Vec<i64> = rows.iter().map(|(id, _)| *id).collect();
        for part in ids.chunks(50) {
            let part = part.to_vec();
            self.write(move |tx| {
                tx.execute(
                    &format!(
                        "UPDATE generation_audit SET image=NULL, image_file=NULL, image_size=NULL, image_type='' \
                         WHERE id IN ({})",
                        marks(part.len())
                    ),
                    rusqlite::params_from_iter(part.iter()),
                )?;
                Ok(())
            })
            .await?;
        }
        let files: Vec<String> = rows
            .into_iter()
            .filter_map(|(_, f)| f.filter(|f| !f.is_empty()))
            .collect();
        if !files.is_empty() {
            let dir = self.image_dir().to_path_buf();
            blocking(move || unlink_images(&dir, &files)).await?;
        }
        Ok(ids.len() as i64)
    }

    /// 删掉目录里没有记录指向的原图文件（崩溃留下的 .tmp、写完没入库的），只动 `older_than` 之前的文件；
    /// 顺手删掉空的、同样够旧的日期目录。返回删掉的文件数。
    pub async fn sweep_orphan_images(&self, older_than: f64) -> Result<i64> {
        let known: std::collections::HashSet<String> = self
            .read(|c| {
                let mut stmt = c.prepare(
                    "SELECT image_file FROM generation_audit WHERE image_file IS NOT NULL",
                )?;
                stmt.query_map([], |r| value::text_or(r, 0, ""))?.collect()
            })
            .await?;
        let dir = self.image_dir().to_path_buf();
        blocking(move || sweep(&dir, &known, older_than)).await
    }
}

/// `st_mtime`（Unix 秒，浮点）。
fn mtime_secs(meta: &std::fs::Metadata) -> Option<f64> {
    let t = meta.modified().ok()?;
    Some(match t.duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => d.as_secs_f64(),
        Err(e) => -e.duration().as_secs_f64(),
    })
}

fn sweep(dir: &Path, known: &std::collections::HashSet<String>, older_than: f64) -> i64 {
    fn walk(
        root: &Path,
        cur: &Path,
        known: &std::collections::HashSet<String>,
        older_than: f64,
        n: &mut i64,
    ) {
        let Ok(entries) = std::fs::read_dir(cur) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = std::fs::metadata(&path) else {
                continue;
            };
            if meta.is_dir() {
                walk(root, &path, known, older_than, n);
            } else if meta.is_file() {
                let Ok(rel) = path.strip_prefix(root) else {
                    continue;
                };
                // as_posix()
                let rel = rel
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("/");
                if !known.contains(&rel)
                    && mtime_secs(&meta).is_some_and(|m| m < older_than)
                    && std::fs::remove_file(&path).is_ok()
                {
                    *n += 1;
                }
            }
        }
    }
    if !dir.is_dir() {
        return 0;
    }
    let mut n = 0;
    walk(dir, dir, known, older_than, &mut n);
    // 空的日期目录顺手删掉；刚建的目录可能正要写图，不碰（rmdir 和写文件之间的竞争）
    let mut subdirs: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|it| it.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    subdirs.sort();
    subdirs.reverse();
    for d in subdirs {
        let Ok(meta) = std::fs::metadata(&d) else {
            continue;
        };
        if !meta.is_dir() {
            continue;
        }
        if mtime_secs(&meta).is_some_and(|m| m >= older_than) {
            continue;
        }
        let _ = std::fs::remove_dir(&d); // 非空会失败，忽略
    }
    n
}
