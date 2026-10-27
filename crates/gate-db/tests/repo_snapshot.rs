//! `Database` 全部方法的快照测试。
//!
//! 每个用例（`tests/snapshots/repo/<领域>.jsonl` 的一行）：新建空库 → 执行 `seed_sql`（以及原图目录快照）→ 调用同名 Rust 方法 →
//! 记录返回值（或错误）、**有变化的表**的整表内容、有变化的原图文件和目录。浮点逐位、JSON 键顺序一致才算相同。
//!
//! 更新：`UPDATE_SNAPSHOT=1 cargo test -p gate-db --test repo_snapshot`，然后审阅 `git diff`。

use base64::{Engine, engine::general_purpose::STANDARD as B64};
use gate_core::{clock::Clock, reasons::reason_label, values};
use gate_db::repo::{
    discord::BanInfo,
    keys::NewKey,
    usage::{LogFilter, NewLog, Success},
};
use gate_db::{Db, DbError};
use gate_snapshot::snapshot::{Suite, path as snapshot_path};
use gate_snapshot::{Case, as_f64, f as fjson};
use rusqlite::types::ValueRef;
use serde::Serialize;
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::{Duration, UNIX_EPOCH};

const M: &str = env!("CARGO_MANIFEST_DIR");

// ------------------------------------------------------------------ 参数解码（输入沿用位置参数 + 关键字参数的写法）
fn arg<'a>(c: &'a Case, i: usize, name: &str) -> Option<&'a Value> {
    c.kwarg(name).or_else(|| c.args().get(i))
}

fn s(v: Option<&Value>, default: &str) -> String {
    v.map_or_else(|| default.to_string(), values::text)
}

fn int(v: Option<&Value>, default: i64) -> i64 {
    match v {
        None | Some(Value::Null) => default,
        // Python int(x)：浮点向零截断
        Some(Value::Number(n)) if n.is_f64() => n.as_f64().unwrap().trunc() as i64,
        Some(other) => values::int_like(other).unwrap_or_else(|| panic!("不是整数：{other}")),
    }
}

fn num(v: Option<&Value>, default: f64) -> f64 {
    match v {
        None | Some(Value::Null) => default,
        Some(other) => as_f64(other).unwrap_or_else(|| panic!("不是数字：{other}")),
    }
}

fn enabled(v: Option<&Value>, default: bool) -> bool {
    v.map_or(default, values::enabled)
}

fn opt_int(v: Option<&Value>) -> Option<i64> {
    match v {
        None | Some(Value::Null) => None,
        other => Some(int(other, 0)),
    }
}

fn str_list(v: &Value) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_str().unwrap().to_string())
        .collect()
}

// ------------------------------------------------------------------ 返回值编码（`$map` / `$b` 的约定见 gate_snapshot）
fn js<T: Serialize>(v: &T) -> Value {
    serde_json::to_value(v).unwrap()
}

/// dict[int, X]：非空时是 {"$map": [[k, v], ...]}，空时是 {}。
fn int_map<V: Serialize>(items: Vec<(i64, V)>) -> Value {
    if items.is_empty() {
        return json!({});
    }
    json!({"$map": items.iter().map(|(k, v)| json!([k, js(v)])).collect::<Vec<_>>()})
}

/// dict[str, X]：按插入顺序的 JSON 对象。
fn str_map<V: Serialize>(items: Vec<(String, V)>) -> Value {
    Value::Object(items.into_iter().map(|(k, v)| (k, js(&v))).collect())
}

fn float(x: f64) -> Value {
    fjson(x)
}

// ------------------------------------------------------------------ 库快照
fn sql_to_json(v: ValueRef<'_>) -> Value {
    match v {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(i) => Value::from(i),
        ValueRef::Real(x) => fjson(x),
        ValueRef::Text(t) => Value::String(String::from_utf8_lossy(t).into_owned()),
        ValueRef::Blob(b) => json!({"$b": B64.encode(b)}),
    }
}

async fn dump_tables(db: &Db) -> Value {
    db.read(|c| {
        let names: Vec<String> = c
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")?
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let mut out = Map::new();
        for name in names {
            let mut stmt = c.prepare(&format!("SELECT * FROM \"{name}\" ORDER BY rowid"))?;
            let cols: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
            let rows: Vec<Value> = stmt
                .query_map([], |r| {
                    let mut m = Map::new();
                    for (i, col) in cols.iter().enumerate() {
                        m.insert(col.clone(), sql_to_json(r.get_ref(i)?));
                    }
                    Ok(Value::Object(m))
                })?
                .collect::<rusqlite::Result<_>>()?;
            if !rows.is_empty() {
                out.insert(name, Value::Array(rows));
            }
        }
        Ok(Value::Object(out))
    })
    .await
    .unwrap()
}

/// 原图目录快照：({相对路径: base64}, [子目录相对路径])。`extra` 是目录之外的文件（相对路径含 ".."）。
fn snap_files(root: &Path, extra: &[String]) -> (Value, Value) {
    let mut files = BTreeMap::new();
    let mut dirs = Vec::new();
    if root.is_file() {
        files.insert(String::new(), B64.encode(std::fs::read(root).unwrap()));
    } else if root.is_dir() {
        fn walk(
            root: &Path,
            cur: &Path,
            files: &mut BTreeMap<String, String>,
            dirs: &mut Vec<String>,
        ) {
            for e in std::fs::read_dir(cur).unwrap().flatten() {
                let p = e.path();
                let rel = p
                    .strip_prefix(root)
                    .unwrap()
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("/");
                if p.is_dir() {
                    dirs.push(rel);
                    walk(root, &p, files, dirs);
                } else if p.is_file() {
                    files.insert(rel, B64.encode(std::fs::read(&p).unwrap()));
                }
            }
        }
        walk(root, root, &mut files, &mut dirs);
    }
    for rel in extra {
        let p = root.join(rel);
        if p.is_file() {
            files.insert(rel.clone(), B64.encode(std::fs::read(p).unwrap()));
        }
    }
    dirs.sort();
    (
        Value::Object(
            files
                .into_iter()
                .map(|(k, v)| (k, Value::String(v)))
                .collect(),
        ),
        json!(dirs),
    )
}

fn set_mtime(p: &Path, secs: f64) {
    let t = UNIX_EPOCH + Duration::from_secs_f64(secs);
    std::fs::File::open(p).unwrap().set_modified(t).unwrap();
}

/// 输入里可省略的数组字段（空数组不写进快照）。
fn list<'a>(case: &'a Case, key: &str) -> &'a [Value] {
    case.get(key)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

async fn seed(db: &Db, case: &Case) -> Vec<String> {
    for ddl in list(case, "extra_ddl") {
        let ddl = ddl.as_str().unwrap().to_string();
        db.write(move |tx| tx.execute_batch(&ddl)).await.unwrap();
    }
    for stmt in list(case, "seed_sql") {
        let stmt = stmt.as_str().unwrap().to_string();
        let sql = stmt.clone();
        db.write(move |tx| tx.execute_batch(&sql))
            .await
            .unwrap_or_else(|e| panic!("种子 SQL 执行失败：{e}\n{stmt:?}"));
    }
    let root = &db
        .path()
        .parent()
        .unwrap()
        .join("audit_images")
        .to_path_buf();
    let mut extra = Vec::new();
    let seed_files = list(case, "seed_files");
    let seed_dirs = list(case, "seed_dirs");
    for d in seed_dirs {
        std::fs::create_dir_all(root.join(d["path"].as_str().unwrap())).unwrap();
    }
    for f in seed_files {
        let rel = f["path"].as_str().unwrap();
        let p = if rel.is_empty() {
            root.clone()
        } else {
            root.join(rel)
        };
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, B64.decode(f["b64"].as_str().unwrap()).unwrap()).unwrap();
        if rel.starts_with("..") {
            extra.push(rel.to_string());
        }
    }
    for f in seed_files {
        let rel = f["path"].as_str().unwrap();
        let p = if rel.is_empty() {
            root.clone()
        } else {
            root.join(rel)
        };
        set_mtime(&p, f["mtime"].as_f64().unwrap());
    }
    // 目录 mtime 必须在所有文件都建好之后再设置
    for d in seed_dirs {
        set_mtime(
            &root.join(d["path"].as_str().unwrap()),
            d["mtime"].as_f64().unwrap(),
        );
    }
    extra
}

// ------------------------------------------------------------------ 逐方法调用
type Out = Result<Value, DbError>;

async fn call(db: &Db, c: &Case) -> Out {
    let now = c.get("now").unwrap().as_f64().unwrap();
    let clock = Clock::new("Asia/Shanghai", 7);
    let a = |i: usize, name: &str| arg(c, i, name);
    let m = c.get("method").unwrap().as_str().unwrap();
    Ok(match m {
        // ---------------- reconciliation
        "reconciliation_totals" => db.reconciliation_totals().await?,
        "save_reconciliation" => {
            db.save_reconciliation(c.arg(0)).await?;
            Value::Null
        }
        "reconciliation_history" => json!(db.reconciliation_history(int(a(0, "limit"), 20)).await?),
        // ---------------- upstream
        "get_upstream_counter" => js(&db
            .get_upstream_counter(&s(a(0, "token_id"), ""), &s(a(1, "day"), ""))
            .await?),
        "move_upstream_token" => {
            db.move_upstream_token(&s(a(0, "old_id"), ""), &s(a(1, "new_id"), ""))
                .await?;
            Value::Null
        }
        "get_upstream_token_limits" => str_map(db.get_upstream_token_limits().await?),
        "get_upstream_token_enabled" => str_map(db.get_upstream_token_enabled().await?),
        "get_upstream_token_image_concurrency" => {
            str_map(db.get_upstream_token_image_concurrency().await?)
        }
        "set_upstream_token_limit" => {
            db.set_upstream_token_limit(&s(a(0, "token_id"), ""), int(a(1, "limit"), 0))
                .await?;
            Value::Null
        }
        "set_upstream_token_enabled" => {
            db.set_upstream_token_enabled(
                &s(a(0, "token_id"), ""),
                enabled(a(1, "enabled"), false),
            )
            .await?;
            Value::Null
        }
        "set_upstream_token_image_concurrency" => {
            db.set_upstream_token_image_concurrency(
                &s(a(0, "token_id"), ""),
                int(a(1, "limit"), 0),
            )
            .await?;
            Value::Null
        }
        "bump_upstream_image_counter" => {
            // 一张 = 一份（没有 weight 参数）
            db.bump_upstream_image_counter(
                &s(a(0, "token_id"), ""),
                &s(a(1, "day"), ""),
                int(a(2, "images"), 0),
            )
            .await?;
            Value::Null
        }
        "bump_upstream_v5_counter" => {
            db.bump_upstream_v5_counter(&s(a(0, "token_id"), ""), &s(a(1, "day"), ""))
                .await?;
            Value::Null
        }
        // ---------------- settings
        "get_setting" => match db.get_setting(&s(a(0, "key"), "")).await? {
            Some(v) => json!(v),
            // 旧版返回调用方给的默认值（任意类型）
            None => a(1, "default").cloned().unwrap_or(Value::Null),
        },
        "set_setting" => {
            db.set_setting(&s(a(0, "key"), ""), &s(a(1, "value"), ""))
                .await?;
            Value::Null
        }
        "set_settings_bulk" => {
            let pairs: Vec<(String, String)> = c
                .arg(0)
                .as_object()
                .unwrap()
                .iter()
                .map(|(k, v)| (k.clone(), values::text(v)))
                .collect();
            db.set_settings_bulk(&pairs).await?;
            Value::Null
        }
        // ---------------- coupons
        "add_coupon" => js(&db
            .add_coupon(
                int(a(0, "key_id"), 0),
                &s(a(1, "kind"), ""),
                num(a(2, "days"), 0.0),
                &s(a(3, "note"), ""),
                &s(a(4, "by"), ""),
                "v45,v5,anlas",
                now,
            )
            .await?),
        "list_coupons" => json!(
            db.list_coupons(
                int(a(0, "key_id"), 0),
                enabled(a(1, "active_only"), true),
                now
            )
            .await?
        ),
        "use_coupon" => {
            json!(
                db.use_coupon(int(a(0, "coupon_id"), 0), int(a(1, "key_id"), 0), now)
                    .await?
            )
        }
        // ---------------- admin_actions
        "add_admin_action" => {
            db.add_admin_action(
                &s(a(0, "actor"), ""),
                &s(a(1, "action"), ""),
                &s(a(2, "target"), ""),
                &s(a(3, "detail"), ""),
                enabled(a(4, "ok"), false),
                now,
            )
            .await?;
            Value::Null
        }
        // ---------------- discord
        "forget_registration_for_key" => {
            json!(
                db.forget_registration_for_key(int(a(0, "key_id"), 0))
                    .await?
            )
        }
        "ban_discord" => {
            let info = BanInfo {
                reason: s(c.kwarg("reason"), ""),
                username: s(c.kwarg("username"), ""),
                display_name: s(c.kwarg("display_name"), ""),
                avatar: s(c.kwarg("avatar"), ""),
                banned_by: s(c.kwarg("banned_by"), "系统"),
            };
            db.ban_discord(&s(a(0, "discord_id"), ""), &info, now)
                .await?;
            Value::Null
        }
        "unban_discord" => json!(db.unban_discord(&s(a(0, "discord_id"), "")).await?),
        "list_bans" => {
            let q = match a(0, "q") {
                None | Some(Value::Null) => String::new(),
                Some(v) => values::text(v),
            };
            json!(db.list_bans(&q).await?)
        }
        // ---------------- keys
        "create_key" => {
            let nk: NewKey = serde_json::from_value(c.arg(0).clone())
                .map_err(|e| DbError::Invalid(format!("入参缺字段：{e}")))?;
            db.create_key(&nk, now).await?
        }
        "get_key" => json!(db.get_key(int(a(0, "key_id"), 0)).await?),
        "get_key_by_token" => json!(db.get_key_by_token(&s(a(0, "token"), "")).await?),
        "rotate_key_token" => {
            json!(
                db.rotate_key_token(int(a(0, "key_id"), 0), &s(a(1, "token"), ""))
                    .await?
            )
        }
        "update_key" => {
            db.update_key(int(a(0, "key_id"), 0), c.arg(1).as_object().unwrap())
                .await?;
            Value::Null
        }
        "delete_key" => {
            db.delete_key(int(a(0, "key_id"), 0), now).await?;
            Value::Null
        }
        // 已过期的 Key 不算闲置，所以要带 `now`
        "inactive_key_ids" => json!(db.inactive_key_ids(num(a(0, "cutoff"), 0.0), now).await?),
        "list_keys" => json!(db.list_keys().await?),
        "touch_key" => {
            db.touch_key(int(a(0, "key_id"), 0), now).await?;
            Value::Null
        }
        "set_key_role" => {
            db.set_key_role(int(a(0, "key_id"), 0), &s(a(1, "role"), ""), now)
                .await?;
            Value::Null
        }
        // ---------------- counters
        "get_counter" => {
            let c = db
                .get_counter(int(a(0, "key_id"), 0), &s(a(1, "day"), ""))
                .await?;
            json!({
                "images": c.images,
                "legacy_free_images": c.legacy_free_images,
                "anlas": c.anlas,
                "text_tokens": c.text_tokens,
                "requests": c.requests,
                "v5": c.v5,
            })
        }
        "reset_daily_image_quota" => {
            db.reset_daily_image_quota(
                int(a(0, "key_id"), 0),
                &s(a(1, "day"), ""),
                gate_db::repo::counters::ResetScope::ALL,
            )
            .await?;
            Value::Null
        }
        "month_anlas" => float(
            db.month_anlas(int(a(0, "key_id"), 0), &s(a(1, "month"), ""))
                .await?,
        ),
        "day_v5_total" => json!(db.day_v5_total(&s(a(0, "day"), "")).await?),
        "day_anlas_total" => float(db.day_anlas_total(&s(a(0, "day"), "")).await?),
        "month_anlas_all" => float(db.month_anlas_all(&s(a(0, "month"), "")).await?),
        "member_usage" => int_map(db.member_usage(&s(a(0, "since_day"), "")).await?),
        "overview" => js(&db
            .overview(&s(a(0, "today"), ""), &str_list(c.arg(1)), now, &clock)
            .await?),
        // ---------------- usage
        "record_success" => {
            let sc = Success {
                key_id: int(a(0, "key_id"), 0),
                key_name: s(a(1, "key_name"), ""),
                kind: s(a(2, "kind"), ""),
                model: s(a(3, "model"), ""),
                day: s(a(4, "day"), ""),
                images: int(c.kwarg("images"), 0),
                anlas: num(c.kwarg("anlas"), 0.0),
                tokens: int(c.kwarg("tokens"), 0),
                v5: int(c.kwarg("v5"), 0),
                legacy_free_images: int(c.kwarg("legacy_free_images"), 0),
                detail: s(c.kwarg("detail"), ""),
                unconfirmed_anlas: num(c.kwarg("unconfirmed_anlas"), 0.0),
                wait_ms: int(c.kwarg("wait_ms"), 0),
                dur_ms: int(c.kwarg("dur_ms"), 0),
                up_status: int(c.kwarg("up_status"), 0),
                rid: s(c.kwarg("rid"), ""),
                token_id: None,
                ..Default::default()
            };
            db.record_success(&sc, now).await?;
            Value::Null
        }
        "add_log" => {
            let l = NewLog {
                key_id: opt_int(a(0, "key_id")),
                key_name: s(a(1, "key_name"), ""),
                kind: s(a(2, "kind"), ""),
                model: s(a(3, "model"), ""),
                status: s(a(4, "status"), ""),
                images: int(a(5, "images"), 0),
                anlas: num(a(6, "anlas"), 0.0),
                tokens: int(a(7, "tokens"), 0),
                detail: s(a(8, "detail"), ""),
                unconfirmed_anlas: num(a(9, "unconfirmed_anlas"), 0.0),
                wait_ms: int(a(10, "wait_ms"), 0),
                dur_ms: int(a(11, "dur_ms"), 0),
                up_status: int(a(13, "up_status"), 0),
                rid: s(a(14, "rid"), ""),
                reason: s(a(16, "reason"), ""),
                token_id: None,
                ..Default::default()
            };
            db.add_log(&l, now).await?;
            Value::Null
        }
        "list_logs" => {
            let fl = LogFilter {
                key_id: opt_int(a(2, "key_id")),
                kinds: a(3, "kinds").filter(|v| !v.is_null()).map(str_list),
                rid: s(a(5, "rid"), ""),
                key_ids: a(6, "key_ids")
                    .filter(|v| !v.is_null())
                    .map(|v| {
                        v.as_array()
                            .unwrap()
                            .iter()
                            .filter_map(Value::as_i64)
                            .collect()
                    })
                    .unwrap_or_default(),
                status: s(a(7, "status"), ""),
                ip: s(a(8, "ip"), ""),
                reason: s(a(9, "reason"), ""),
            };
            json!(
                db.list_logs(int(a(0, "limit"), 20), int(a(1, "offset"), 0), &fl)
                    .await?
            )
        }
        "log_summary" => {
            let fl = LogFilter {
                key_id: opt_int(a(0, "key_id")),
                kinds: a(1, "kinds").filter(|v| !v.is_null()).map(str_list),
                rid: s(a(3, "rid"), ""),
                key_ids: a(4, "key_ids")
                    .filter(|v| !v.is_null())
                    .map(|v| {
                        v.as_array()
                            .unwrap()
                            .iter()
                            .filter_map(Value::as_i64)
                            .collect()
                    })
                    .unwrap_or_default(),
                status: s(a(5, "status"), ""),
                ip: s(a(6, "ip"), ""),
                reason: s(a(7, "reason"), ""),
            };
            json!(db.log_summary(&fl).await?)
        }
        "purge_usage_log" => json!(db.purge_usage_log(num(a(0, "older_than"), 0.0)).await?),
        "member_milestones" => int_map(
            db.member_milestones(num(a(0, "since"), 0.0), &reason_label)
                .await?,
        ),
        "image_perf_rows" => json!(
            db.image_perf_rows(num(a(0, "since"), 0.0))
                .await?
                .into_iter()
                .map(|(ts, model, status, images, wait, dur, up, detail)| json!([
                    float(ts),
                    model,
                    status,
                    images,
                    wait,
                    dur,
                    up,
                    detail
                ]))
                .collect::<Vec<_>>()
        ),
        "image_stability" => js(&db.image_stability(num(a(0, "since"), 0.0)).await?),
        "usage_by_kind" => json!(db.usage_by_kind(num(a(0, "since"), 0.0)).await?),
        "generated_image_totals" => {
            int_map(db.generated_image_totals(opt_int(a(0, "key_id"))).await?)
        }
        "user_image_timing" => js(&db.user_image_timing(int(a(0, "key_id"), 0), now).await?),
        other => panic!("夹具里有未实现的方法：{other}"),
    })
}

// ------------------------------------------------------------------ 驱动

/// 两次表快照之间有变化的表：值是调用后该表的整表内容（表被清空时是空数组）。
fn changed_tables(before: &Value, after: &Value) -> Map<String, Value> {
    let (before, after) = (before.as_object().unwrap(), after.as_object().unwrap());
    let names: BTreeSet<&String> = before.keys().chain(after.keys()).collect();
    let mut out = Map::new();
    for name in names {
        if before.get(name) != after.get(name) {
            out.insert(
                name.clone(),
                after.get(name).cloned().unwrap_or_else(|| json!([])),
            );
        }
    }
    out
}

/// 两次原图目录快照之间有变化的文件（新增 / 改写：base64 内容；删除：null）。
fn changed_files(before: &Value, after: &Value) -> Map<String, Value> {
    let (before, after) = (before.as_object().unwrap(), after.as_object().unwrap());
    let mut out = Map::new();
    for (k, v) in after {
        if before.get(k) != Some(v) {
            out.insert(k.clone(), v.clone());
        }
    }
    for k in before.keys().filter(|k| !after.contains_key(*k)) {
        out.insert(k.clone(), Value::Null);
    }
    out
}

/// 一个用例的输出：`ret`（返回值）或 `err`，加上有变化的 `tables` / `files`，子目录有变化时再加 `dirs`（调用后的完整列表）。
async fn run_case(case: &Case) -> Value {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path().join("gate.db")).unwrap();
    let extra = seed(&db, case).await;
    let tables_before = dump_tables(&db).await;
    let (files_before, dirs_before) =
        snap_files(&db.path().parent().unwrap().join("audit_images"), &extra);

    let mut out = Map::new();
    match call(&db, case).await {
        Ok(v) => out.insert("ret".into(), v),
        Err(e) => {
            // 错误文案里可能带临时目录路径
            let msg = e.to_string().replace(dir.path().to_str().unwrap(), "<tmp>");
            out.insert("err".into(), json!(msg))
        }
    };
    let tables = changed_tables(&tables_before, &dump_tables(&db).await);
    if !tables.is_empty() {
        out.insert("tables".into(), Value::Object(tables));
    }
    let (files_after, dirs_after) =
        snap_files(&db.path().parent().unwrap().join("audit_images"), &extra);
    let files = changed_files(&files_before, &files_after);
    if !files.is_empty() {
        out.insert("files".into(), Value::Object(files));
    }
    if dirs_before != dirs_after {
        out.insert("dirs".into(), dirs_after);
    }
    Value::Object(out)
}

async fn run_area(area: &str) {
    let mut suite = Suite::open(M, &format!("repo/{area}"));
    let cases = suite.cases();
    for (i, case) in cases.iter().enumerate() {
        suite.record(i, Some(run_case(case).await));
    }
    suite.finish();
}

macro_rules! area_tests {
    ($($name:ident),* $(,)?) => {
        const AREAS: &[&str] = &[$(stringify!($name)),*];
        $(
            #[tokio::test(flavor = "multi_thread")]
            async fn $name() {
                run_area(stringify!($name)).await;
            }
        )*
    };
}

area_tests!(
    reconciliation,
    upstream,
    settings,
    coupons,
    admin_actions,
    discord,
    keys,
    counters,
    usage,
);

/// `call` 支持的全部方法：每个方法在快照里至少要有 `MIN_CASES_PER_METHOD` 个用例（抽样、删用例时不会悄悄丢掉一个方法）。
const METHODS: &[&str] = &[
    "reconciliation_totals",
    "save_reconciliation",
    "reconciliation_history",
    "get_upstream_counter",
    "move_upstream_token",
    "get_upstream_token_limits",
    "get_upstream_token_enabled",
    "get_upstream_token_image_concurrency",
    "set_upstream_token_limit",
    "set_upstream_token_enabled",
    "set_upstream_token_image_concurrency",
    "bump_upstream_image_counter",
    "bump_upstream_v5_counter",
    "get_setting",
    "set_setting",
    "set_settings_bulk",
    "add_coupon",
    "list_coupons",
    "use_coupon",
    "add_admin_action",
    "forget_registration_for_key",
    "ban_discord",
    "unban_discord",
    "list_bans",
    "create_key",
    "get_key",
    "get_key_by_token",
    "rotate_key_token",
    "update_key",
    "delete_key",
    "inactive_key_ids",
    "list_keys",
    "touch_key",
    "set_key_role",
    "get_counter",
    "reset_daily_image_quota",
    "month_anlas",
    "day_v5_total",
    "day_anlas_total",
    "month_anlas_all",
    "member_usage",
    "overview",
    "record_success",
    "add_log",
    "list_logs",
    "log_summary",
    "purge_usage_log",
    "member_milestones",
    "image_perf_rows",
    "image_stability",
    "usage_by_kind",
    "generated_image_totals",
    "user_image_timing",
];
const MIN_CASES_PER_METHOD: usize = 2;

#[test]
fn every_method_keeps_a_few_cases() {
    let mut count: BTreeMap<String, usize> = BTreeMap::new();
    for area in AREAS {
        let path = snapshot_path(M, &format!("repo/{area}"));
        let text = std::fs::read_to_string(&path).unwrap();
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            let v: Value = serde_json::from_str(line).unwrap();
            *count
                .entry(v["in"]["method"].as_str().unwrap().to_string())
                .or_default() += 1;
        }
    }
    for m in METHODS {
        let n = count.remove(*m).unwrap_or(0);
        assert!(
            n >= MIN_CASES_PER_METHOD,
            "方法 {m} 在快照里只有 {n} 个用例（至少 {MIN_CASES_PER_METHOD} 个）"
        );
    }
    assert!(count.is_empty(), "快照里有 `call` 不认识的方法：{count:?}");
}
