//! 防一人多号 / 共用 Key 的「足迹」（站长 10-23）：网页登录时读到的 Discord 服务器列表和绑定的外部账号。
//! 设备指纹补充项和电量快照。
//! 只记录、不自动处理；关联由每 6 小时的扫描任务写进 `abuse_signals`。永久保留。

use crate::{Db, Result};
use gate_core::primitives::take_chars;
use rusqlite::OptionalExtension;
use rusqlite::params;

/// Discord 绑定的外部账号（`GET /users/@me/connections` 的一项）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiscordConnection {
    /// 平台：steam / github / twitch / xbox …
    pub kind: String,
    /// 平台上的账号 ID
    pub id: String,
    pub name: String,
    pub verified: bool,
}

/// 一张参考图的哈希（`ref_images` 一行）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefImage {
    /// img2img / vibe / precise_ref / vibe_encode
    pub kind: &'static str,
    pub sha256: String,
    /// 16 位十六进制；解不出图为 None
    pub dhash: Option<String>,
}

/// 电量快照：level 0–1，是否在充电。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Battery {
    pub level: f64,
    pub charging: bool,
}

impl Db {
    /// 记下一次请求里的参考图哈希。
    pub async fn record_ref_images(
        &self,
        rid: &str,
        key_id: i64,
        images: &[RefImage],
        now: f64,
    ) -> Result<()> {
        if images.is_empty() {
            return Ok(());
        }
        let (rid, images) = (rid.to_string(), images.to_vec());
        self.write(move |tx| {
            let mut st = tx.prepare(
                "INSERT INTO ref_images(rid, key_id, ts, kind, sha256, dhash) VALUES (?,?,?,?,?,?)",
            )?;
            for i in &images {
                st.execute(params![rid, key_id, now, i.kind, i.sha256, i.dhash])?;
            }
            Ok(())
        })
        .await
    }

    /// 存设备指纹补充项（同一个设备指纹 + 浏览器标记只留最新一份）；带了电量就一起记下电量和时刻。
    pub async fn upsert_device_extra(
        &self,
        device_id: &str,
        browser_id: &str,
        data: &str,
        xid: &str,
        battery: Option<Battery>,
        now: f64,
    ) -> Result<()> {
        let (dev, bid, data, xid) = (
            device_id.to_string(),
            browser_id.to_string(),
            data.to_string(),
            xid.to_string(),
        );
        let (level, charging, bts) = match battery {
            Some(b) => (Some(b.level), Some(i64::from(b.charging)), Some(now)),
            None => (None, None, None),
        };
        self.write(move |tx| {
            tx.execute(
                "INSERT INTO device_fp_extra(device_id, browser_id, data, xid, battery_level, battery_charging, battery_ts, ts) \
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(device_id, browser_id) DO UPDATE SET \
                 data=excluded.data, xid=excluded.xid, battery_level=excluded.battery_level, \
                 battery_charging=excluded.battery_charging, battery_ts=excluded.battery_ts, ts=excluded.ts",
                params![dev, bid, data, xid, level, charging, bts, now],
            )?;
            Ok(())
        })
        .await
    }

    /// 网页登录：把这个设备指纹 + 浏览器标记最近一次上报的电量（`since` 之后的）记成这个账号的一条电量快照。
    /// 返回是否记了。
    pub async fn record_login_battery(
        &self,
        discord_id: &str,
        device_id: &str,
        browser_id: &str,
        since: f64,
    ) -> Result<bool> {
        let (d, dev, bid) = (
            discord_id.to_string(),
            device_id.to_string(),
            browser_id.to_string(),
        );
        self.write(move |tx| {
            let row: Option<(f64, f64, i64)> = tx
                .query_row(
                    "SELECT battery_ts, battery_level, battery_charging FROM device_fp_extra \
                     WHERE device_id=? AND browser_id=? AND battery_ts>=? AND battery_level IS NOT NULL",
                    params![dev, bid, since],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()?;
            let Some((ts, level, charging)) = row else {
                return Ok(false);
            };
            tx.execute(
                "INSERT INTO device_battery(discord_id, ts, level, charging) VALUES (?,?,?,?)",
                params![d, ts, level, charging],
            )?;
            Ok(true)
        })
        .await
    }

    /// 记下这次登录看到的服务器列表 `(guild_id, name)`：已有的刷新名字和 last_ts，新的补一行。
    pub async fn record_discord_guilds(
        &self,
        discord_id: &str,
        guilds: &[(String, String)],
        now: f64,
    ) -> Result<()> {
        let d = discord_id.to_string();
        let rows: Vec<(String, String)> = guilds
            .iter()
            .filter(|(g, _)| !g.is_empty())
            .take(500)
            .map(|(g, n)| {
                (
                    take_chars(g, 32).to_string(),
                    take_chars(n, 100).to_string(),
                )
            })
            .collect();
        self.write(move |tx| {
            let mut st = tx.prepare(
                "INSERT INTO discord_user_guilds(discord_id, guild_id, name, first_ts, last_ts) \
                 VALUES (?1,?2,?3,?4,?4) ON CONFLICT(discord_id, guild_id) \
                 DO UPDATE SET name=excluded.name, last_ts=excluded.last_ts",
            )?;
            for (g, n) in &rows {
                st.execute(params![d, g, n, now])?;
            }
            Ok(())
        })
        .await
    }

    /// 记下这次登录看到的外部账号：已有的刷新名字 / 验证状态和 last_ts，新的补一行。
    pub async fn record_discord_connections(
        &self,
        discord_id: &str,
        conns: &[DiscordConnection],
        now: f64,
    ) -> Result<()> {
        let d = discord_id.to_string();
        let rows: Vec<DiscordConnection> = conns
            .iter()
            .filter(|c| !c.kind.is_empty() && !c.id.is_empty())
            .take(100)
            .map(|c| DiscordConnection {
                kind: take_chars(&c.kind, 32).to_string(),
                id: take_chars(&c.id, 128).to_string(),
                name: take_chars(&c.name, 100).to_string(),
                verified: c.verified,
            })
            .collect();
        self.write(move |tx| {
            let mut st = tx.prepare(
                "INSERT INTO discord_user_connections(discord_id, type, ext_id, name, verified, first_ts, last_ts) \
                 VALUES (?1,?2,?3,?4,?5,?6,?6) ON CONFLICT(discord_id, type, ext_id) \
                 DO UPDATE SET name=excluded.name, verified=excluded.verified, last_ts=excluded.last_ts",
            )?;
            for c in &rows {
                st.execute(params![d, c.kind, c.id, c.name, i64::from(c.verified), now])?;
            }
            Ok(())
        })
        .await
    }
}
