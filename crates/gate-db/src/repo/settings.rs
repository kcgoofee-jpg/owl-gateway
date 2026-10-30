//! site_settings 键值表。值一律以字符串存取（写进去什么读出来就是什么）。

use crate::{Db, Result};
use rusqlite::{OptionalExtension, params};

impl Db {
    /// 读设置；没有这一行返回 None。
    pub async fn get_setting(&self, key: &str) -> Result<Option<String>> {
        let key = key.to_string();
        self.read(move |c| {
            c.query_row("SELECT value FROM site_settings WHERE key=?", [key], |r| {
                crate::value::text_or(r, 0, "")
            })
            .optional()
        })
        .await
    }

    /// 写设置。值必须是字符串：调用方负责把数字 / 布尔转成约定的字符串形式
    /// （浮点用 `primitives::float_text`，布尔用 `primitives::bool_text`）。
    pub async fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        let (key, value) = (key.to_string(), value.to_string());
        self.write(move |tx| {
            tx.execute(
                "INSERT INTO site_settings (key, value) VALUES (?,?) \
                 ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                params![key, value],
            )?;
            Ok(())
        })
        .await
    }

    /// 一个事务里写入一组已校验的运行时开关（按给定顺序）。
    pub async fn set_settings_bulk(&self, values: &[(String, String)]) -> Result<()> {
        let values = values.to_vec();
        self.write(move |tx| {
            let mut stmt = tx.prepare(
                "INSERT INTO site_settings (key, value) VALUES (?,?) \
                 ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            )?;
            for (k, v) in &values {
                stmt.execute(params![k, v])?;
            }
            Ok(())
        })
        .await
    }
}
