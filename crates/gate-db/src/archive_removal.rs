//! Retire creative-content archives after a verified, private recovery backup.
use crate::{DbError, Result};
use rusqlite::Connection;
use std::path::Path;

const VACUUM_PENDING: &str = "records_removal_v1_vacuum_pending";

fn copy_tree(source: &Path, target: &Path) -> std::io::Result<()> {
    if std::fs::symlink_metadata(source)?.file_type().is_symlink() {
        return Err(std::io::Error::other(
            "image archive contains a symbolic link",
        ));
    }
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

fn verify(path: &Path) -> Result<()> {
    let c = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let integrity: String = c.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
    if integrity != "ok" {
        return Err(DbError::Invalid(
            "archive removal backup integrity check failed".into(),
        ));
    }
    Ok(())
}

pub fn run(conn: &mut Connection, path: &Path) -> Result<()> {
    let archived: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name IN ('generation_audit','audit_request_links'))",
        [], |r| r.get(0),
    )?;
    let root = path.parent().unwrap_or_else(|| Path::new("."));
    let images = root.join("audit_images");
    let has_images = images.try_exists()? || images.is_symlink();
    let backup = root.join(format!(
        "{}.before-records-removal",
        path.file_name().unwrap_or_default().to_string_lossy()
    ));
    if archived || has_images {
        if backup.is_symlink() {
            return Err(std::io::Error::other("backup directory is a symbolic link").into());
        }
        std::fs::create_dir_all(&backup)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&backup, std::fs::Permissions::from_mode(0o700))?;
        }
        let database = backup.join("database.sqlite");
        if !backup.join("complete").exists() {
            if database.exists() || database.is_symlink() {
                std::fs::remove_file(&database)?;
            }
            conn.execute("VACUUM INTO ?", [database.to_string_lossy().as_ref()])?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&database, std::fs::Permissions::from_mode(0o600))?;
            }
            verify(&database)?;
            if has_images {
                copy_tree(&images, &backup.join("audit_images"))?;
            }
            std::fs::write(
                backup.join("complete"),
                b"database integrity checked; archive copy complete\n",
            )?;
        }
        // Validate the existing recovery archive again before any retry deletes data.
        verify(&database)?;
        // Finish fallible filesystem cleanup before dropping prompt rows. A failure keeps
        // live metadata intact; the offline copy makes partial file cleanup recoverable.
        if has_images {
            if images.is_symlink() {
                return Err(
                    std::io::Error::other("image archive directory is a symbolic link").into(),
                );
            }
            std::fs::remove_dir_all(&images)?;
        }
    }
    let tx = conn.transaction()?;
    tx.execute_batch("DROP TABLE IF EXISTS audit_request_links;
        DROP TABLE IF EXISTS generation_audit;
        DELETE FROM abuse_signals WHERE kind='behavior' AND instr(detail,'冷门负面串')>0;
        DELETE FROM site_settings WHERE key IN ('audit_prompts','audit_thumbs','audit_retention_days','audit_image_cap_mb','audit_image_keep_hours','prompts_only_v1_vacuum_pending');")?;
    if archived {
        tx.execute(
            "INSERT OR REPLACE INTO site_settings(key,value) VALUES(?,'1')",
            [VACUUM_PENDING],
        )?;
    }
    tx.commit()?;
    let compact: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM site_settings WHERE key=?)",
        [VACUUM_PENDING],
        |r| r.get(0),
    )?;
    if compact {
        conn.execute_batch("VACUUM;")?;
        conn.execute("DELETE FROM site_settings WHERE key=?", [VACUUM_PENDING])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn legacy(path: &Path) -> Connection {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch("CREATE TABLE site_settings(key TEXT PRIMARY KEY,value TEXT);
            CREATE TABLE abuse_signals(kind TEXT,detail TEXT);
            INSERT INTO abuse_signals VALUES('behavior','冷门负面串'),('same_browser','keep');
            CREATE TABLE generation_audit(id INTEGER PRIMARY KEY, prompt TEXT, extra TEXT, image BLOB);
            INSERT INTO generation_audit VALUES(1,'synthetic prompt','{\"seed\":7}',X'0102');
            CREATE TABLE audit_request_links(request_id TEXT, audit_id INTEGER);
            CREATE TABLE usage_log(id INTEGER PRIMARY KEY, images INTEGER);
            INSERT INTO usage_log VALUES(1,2);
            CREATE TABLE request_settlements(request_id TEXT PRIMARY KEY, status TEXT);
            INSERT INTO request_settlements VALUES('r1','committed');").unwrap();
        conn
    }

    fn exists(c: &Connection, name: &str) -> bool {
        c.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?)",
            [name],
            |r| r.get(0),
        )
        .unwrap()
    }

    #[test]
    fn verified_backup_then_remove_records_and_images_repeatably() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gate.db");
        let mut c = legacy(&path);
        let images = dir.path().join("audit_images");
        std::fs::create_dir(&images).unwrap();
        std::fs::write(images.join("one.png"), b"synthetic-image").unwrap();
        run(&mut c, &path).unwrap();
        assert!(!exists(&c, "generation_audit"));
        assert!(!exists(&c, "audit_request_links"));
        assert!(!images.exists());
        assert_eq!(
            c.query_row::<i64, _, _>("SELECT images FROM usage_log", [], |r| r.get(0))
                .unwrap(),
            2
        );
        assert_eq!(
            c.query_row::<String, _, _>("SELECT status FROM request_settlements", [], |r| r.get(0))
                .unwrap(),
            "committed"
        );
        assert_eq!(
            c.query_row::<String, _, _>("SELECT kind FROM abuse_signals", [], |r| r.get(0))
                .unwrap(),
            "same_browser"
        );
        let backup = dir.path().join("gate.db.before-records-removal");
        assert_eq!(
            std::fs::read(backup.join("audit_images/one.png")).unwrap(),
            b"synthetic-image"
        );
        let original = Connection::open(backup.join("database.sqlite")).unwrap();
        assert_eq!(
            original
                .query_row::<String, _, _>("SELECT prompt FROM generation_audit", [], |r| r.get(0))
                .unwrap(),
            "synthetic prompt"
        );
        run(&mut c, &path).unwrap();
        assert!(!exists(&c, "generation_audit"));
    }

    #[test]
    fn failed_backup_preserves_live_records() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gate.db");
        let mut c = legacy(&path);
        std::fs::write(
            dir.path().join("gate.db.before-records-removal"),
            b"blocked",
        )
        .unwrap();
        assert!(run(&mut c, &path).is_err());
        assert!(exists(&c, "generation_audit"));
    }

    #[test]
    fn corrupted_backup_is_rejected_before_deletion() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gate.db");
        let mut c = legacy(&path);
        let backup = dir.path().join("gate.db.before-records-removal");
        std::fs::create_dir(&backup).unwrap();
        std::fs::write(backup.join("complete"), b"old marker").unwrap();
        std::fs::write(backup.join("database.sqlite"), b"invalid database").unwrap();
        assert!(run(&mut c, &path).is_err());
        assert!(exists(&c, "generation_audit"));
    }

    #[cfg(unix)]
    #[test]
    fn failed_image_cleanup_preserves_records_and_can_retry() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gate.db");
        let mut c = legacy(&path);
        let outside = dir.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("file"), b"keep").unwrap();
        let images = dir.path().join("audit_images");
        symlink(&outside, &images).unwrap();
        assert!(run(&mut c, &path).is_err());
        assert!(exists(&c, "generation_audit"));
        assert!(outside.join("file").exists());
        std::fs::remove_file(&images).unwrap();
        run(&mut c, &path).unwrap();
        assert!(!exists(&c, "generation_audit"));
        assert!(outside.join("file").exists());
    }
}
