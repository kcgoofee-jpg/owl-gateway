//! 后台操作记录。

use crate::{Db, Result};
use rusqlite::{OptionalExtension, params};

impl Db {
    pub async fn add_admin_action(
        &self,
        actor: &str,
        action: &str,
        target: &str,
        detail: &str,
        ok: bool,
        now: f64,
    ) -> Result<()> {
        let (actor, action, target, detail) = (
            actor.to_string(),
            action.to_string(),
            target.to_string(),
            detail.to_string(),
        );
        self.write(move |tx| {
            tx.execute(
                "INSERT INTO admin_actions (ts, actor, action, target, detail, ok) VALUES (?,?,?,?,?,?)",
                params![now, actor, action, target, detail, i64::from(ok)],
            )?;
            Ok(())
        })
        .await
    }

    /// 手动改一项设置并在**同一个事务**里写操作日志：`value` 为 `None` 时删掉这项设置。
    /// 返回改之前的值。给「手动（Claude）」这类没有专门后台页面的改动用，改动和记录要么都在、要么都不在。
    #[allow(clippy::too_many_arguments)]
    pub async fn set_setting_logged(
        &self,
        key: &str,
        value: Option<&str>,
        actor: &str,
        action: &str,
        target: &str,
        detail: &str,
        now: f64,
    ) -> Result<Option<String>> {
        let (key, value) = (key.to_string(), value.map(str::to_string));
        let (actor, action, target, detail) = (
            actor.to_string(),
            action.to_string(),
            target.to_string(),
            detail.to_string(),
        );
        self.write(move |tx| {
            let old: Option<String> = tx
                .query_row(
                    "SELECT value FROM site_settings WHERE key=?",
                    [&key],
                    |r| r.get(0),
                )
                .optional()?;
            match &value {
                Some(v) => tx.execute(
                    "INSERT INTO site_settings (key, value) VALUES (?,?) \
                     ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                    params![key, v],
                )?,
                None => tx.execute("DELETE FROM site_settings WHERE key=?", [&key])?,
            };
            tx.execute(
                "INSERT INTO admin_actions (ts, actor, action, target, detail, ok) VALUES (?,?,?,?,?,1)",
                params![now, actor, action, target, detail],
            )?;
            Ok(old)
        })
        .await
    }
}
