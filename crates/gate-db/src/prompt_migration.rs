//! One-time migration from image archives to prompt metadata.
use crate::Result;
use rusqlite::{Connection, params};
use std::path::Path;

fn copy_tree(source: &Path, target: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(target)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            return Err(std::io::Error::other(
                "image archive contains a symbolic link",
            ));
        }
        if kind.is_dir() {
            copy_tree(&entry.path(), &target.join(entry.file_name()))?;
        } else if kind.is_file() {
            std::fs::copy(entry.path(), target.join(entry.file_name()))?;
        }
    }
    Ok(())
}

pub fn run(conn: &mut Connection, path: &Path) -> Result<()> {
    let columns: Vec<String> = conn
        .prepare("PRAGMA table_info(generation_audit)")?
        .query_map([], |row| row.get(1))?
        .collect::<rusqlite::Result<_>>()?;
    let old = columns.iter().any(|c| c == "image" || c == "thumb");
    let root = path.parent().unwrap_or_else(|| Path::new("."));
    let images = root.join("audit_images");
    let backup = root.join(format!(
        "{}.before-prompts-only",
        path.file_name().unwrap_or_default().to_string_lossy()
    ));
    if (old || images.exists()) && !backup.join("complete").exists() {
        std::fs::create_dir_all(&backup)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&backup, std::fs::Permissions::from_mode(0o700))?;
        }
        let database = backup.join("database.sqlite");
        if database.exists() {
            std::fs::remove_file(&database)?;
        }
        conn.execute("VACUUM INTO ?", [database.to_string_lossy().as_ref()])?;
        let check = Connection::open(&database)?;
        let integrity: String = check.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
        if integrity != "ok" {
            return Err(crate::DbError::Invalid(
                "migration backup integrity check failed".into(),
            ));
        }
        if images.exists() {
            copy_tree(&images, &backup.join("audit_images"))?;
        }
        std::fs::write(
            backup.join("complete"),
            b"database integrity checked; image copy complete\n",
        )?;
    }
    if old {
        let tx = conn.transaction()?;
        tx.execute_batch("CREATE TABLE prompt_metadata_migration (
            id INTEGER PRIMARY KEY AUTOINCREMENT, ts REAL NOT NULL, key_id INTEGER,
            key_name TEXT NOT NULL DEFAULT '', kind TEXT NOT NULL DEFAULT 'image',
            model TEXT NOT NULL DEFAULT '', status TEXT NOT NULL DEFAULT '',
            prompt TEXT NOT NULL DEFAULT '', negative TEXT NOT NULL DEFAULT '',
            extra TEXT NOT NULL DEFAULT '', rid TEXT NOT NULL DEFAULT '', request_id TEXT NOT NULL DEFAULT '');")?;
        let links: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='audit_request_links')", [], |r| r.get(0))?;
        let request_id = if links {
            "COALESCE((SELECT request_id FROM audit_request_links WHERE audit_id=generation_audit.id),'')"
        } else {
            "''"
        };
        tx.execute_batch(&format!("INSERT INTO prompt_metadata_migration SELECT id,ts,key_id,key_name,kind,model,status,prompt,negative,extra,rid,{request_id} FROM generation_audit;
            DROP TABLE generation_audit;
            ALTER TABLE prompt_metadata_migration RENAME TO generation_audit;
            CREATE INDEX idx_audit_ts ON generation_audit(ts);
            CREATE INDEX idx_audit_key ON generation_audit(key_id,ts);
            CREATE INDEX idx_audit_key_id ON generation_audit(key_id,id DESC);"))?;
        tx.execute("INSERT OR REPLACE INTO site_settings(key,value) VALUES('prompts_only_v1_vacuum_pending','1')", [])?;
        tx.commit()?;
    }
    let compact: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM site_settings WHERE key='prompts_only_v1_vacuum_pending')",
        [],
        |r| r.get(0),
    )?;
    if compact {
        conn.execute_batch("VACUUM;")?;
        conn.execute(
            "DELETE FROM site_settings WHERE key='prompts_only_v1_vacuum_pending'",
            [],
        )?;
    }
    // File deletion is retried on startup, after the verified backup and database migration.
    if images.exists() {
        if std::fs::symlink_metadata(&images)?.file_type().is_symlink() {
            return Err(std::io::Error::other("image archive directory is a symbolic link").into());
        }
        std::fs::remove_dir_all(&images)?;
    }
    let tx = conn.transaction()?;
    tx.execute_batch("DROP TABLE IF EXISTS audit_request_links;
        DELETE FROM site_settings WHERE key IN ('audit_prompts','audit_thumbs','audit_retention_days','audit_image_cap_mb','audit_image_keep_hours');")?;
    tx.execute("DELETE FROM generation_audit WHERE prompt='' AND negative='' AND (extra='' OR (CASE WHEN json_valid(extra) THEN COALESCE(json_array_length(extra,'$.char_prompts'),0)=0 AND COALESCE(json_array_length(extra,'$.char_negatives'),0)=0 AND NOT EXISTS(SELECT 1 FROM json_each(extra,'$.params')) ELSE 0 END))", [])?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();
    tx.execute(
        "DELETE FROM generation_audit WHERE ts<?",
        params![now - gate_core::PROMPT_RETENTION_DAYS as f64 * 86400.0],
    )?;
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn legacy(path: &Path) -> Connection {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch("CREATE TABLE site_settings(key TEXT PRIMARY KEY,value TEXT);
          CREATE TABLE generation_audit(id INTEGER PRIMARY KEY AUTOINCREMENT,ts REAL NOT NULL,key_id INTEGER,key_name TEXT NOT NULL DEFAULT '',kind TEXT NOT NULL DEFAULT 'image',model TEXT NOT NULL DEFAULT '',status TEXT NOT NULL DEFAULT '',prompt TEXT NOT NULL DEFAULT '',negative TEXT NOT NULL DEFAULT '',extra TEXT NOT NULL DEFAULT '',rid TEXT NOT NULL DEFAULT '',thumb BLOB,image BLOB,image_type TEXT,image_file TEXT,image_size INTEGER);
          CREATE TABLE audit_request_links(request_id TEXT PRIMARY KEY,audit_id INTEGER UNIQUE);
          INSERT INTO site_settings VALUES('audit_prompts','0');").unwrap();
        conn
    }
    #[test]
    fn backup_preserves_images_and_migration_preserves_recent_prompts() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gate.db");
        let mut conn = legacy(&path);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();
        for (id, ts) in [(1, now - 29.0 * 86400.0), (2, now - 31.0 * 86400.0)] {
            conn.execute("INSERT INTO generation_audit(id,ts,prompt,negative,extra,thumb,image) VALUES(?,?,'positive','negative','{\"params\":{\"seed\":7}}',X'01',X'0203')",params![id,ts]).unwrap();
        }
        conn.execute("INSERT INTO audit_request_links VALUES('request-1',1)", [])
            .unwrap();
        let images = dir.path().join("audit_images");
        std::fs::create_dir(&images).unwrap();
        std::fs::write(images.join("one.png"), b"synthetic-image").unwrap();
        run(&mut conn, &path).unwrap();
        assert!(!images.exists());
        let backup = dir.path().join("gate.db.before-prompts-only");
        assert_eq!(
            std::fs::read(backup.join("audit_images/one.png")).unwrap(),
            b"synthetic-image"
        );
        let original = Connection::open(backup.join("database.sqlite")).unwrap();
        assert_eq!(
            original
                .query_row("SELECT image FROM generation_audit WHERE id=1", [], |r| r
                    .get::<_, Vec<
                    u8,
                >>(
                    0
                ))
                .unwrap(),
            vec![2, 3]
        );
        assert_eq!(
            conn.query_row(
                "SELECT prompt||':'||request_id FROM generation_audit",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            "positive:request-1"
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM generation_audit", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
        run(&mut conn, &path).unwrap();
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM generation_audit", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert!(conn.prepare("SELECT image FROM generation_audit").is_err());
    }
    #[cfg(unix)]
    #[test]
    fn failed_file_cleanup_keeps_prompts_and_is_retryable() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gate.db");
        let mut conn = legacy(&path);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();
        conn.execute(
            "INSERT INTO generation_audit(ts,prompt) VALUES(?,'keep')",
            [now],
        )
        .unwrap();
        let outside = dir.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("file"), b"keep").unwrap();
        symlink(&outside, dir.path().join("audit_images")).unwrap();
        assert!(run(&mut conn, &path).is_err());
        assert_eq!(
            conn.query_row("SELECT prompt FROM generation_audit", [], |r| r
                .get::<_, String>(0))
                .unwrap(),
            "keep"
        );
        assert!(outside.join("file").exists());
        std::fs::remove_file(dir.path().join("audit_images")).unwrap();
        run(&mut conn, &path).unwrap();
        assert_eq!(
            conn.query_row("SELECT prompt FROM generation_audit", [], |r| r
                .get::<_, String>(0))
                .unwrap(),
            "keep"
        );
    }
}
