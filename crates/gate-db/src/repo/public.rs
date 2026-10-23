//! 公开页面（首页首屏 / 实时看板）用到的两条查询。

use crate::{Db, Result, value};
use rusqlite::OptionalExtension;

/// 领取记录里的 Discord 资料：(username, display_name, avatar)，任一项可能为 NULL。
pub type DiscordProfileRow = (Option<String>, Option<String>, Option<String>);

impl Db {
    /// 首页「我」里的 Discord 资料：没有登记行返回 None。
    pub async fn discord_profile_row(&self, discord_id: &str) -> Result<Option<DiscordProfileRow>> {
        let id = discord_id.to_string();
        self.read(move |c| {
            let mut stmt = c.prepare(
                "SELECT username, display_name, avatar FROM discord_registrations WHERE discord_id=?",
            )?;
            let mut rows = stmt.query([&id])?;
            if let Some(r) = rows.next()? {
                return Ok(Some((
                    value::text_opt(r, 0)?,
                    value::text_opt(r, 1)?,
                    value::text_opt(r, 2)?,
                )));
            }
            // 没领过 Key：用网页登录时记下的资料
            let mut stmt = c.prepare(
                "SELECT username, display_name, avatar FROM web_profiles WHERE discord_id=?",
            )?;
            let mut rows = stmt.query([&id])?;
            Ok(match rows.next()? {
                Some(r) => Some((
                    value::text_opt(r, 0)?,
                    value::text_opt(r, 1)?,
                    value::text_opt(r, 2)?,
                )),
                None => None,
            })
        })
        .await
    }

    /// 记下网页登录时 Discord 给的名字 / 头像（每次登录刷新）。
    pub async fn upsert_web_profile(
        &self,
        discord_id: &str,
        username: &str,
        display_name: &str,
        avatar: &str,
        now: f64,
    ) -> Result<()> {
        let (id, u, d, a) = (
            discord_id.to_string(),
            username.to_string(),
            display_name.to_string(),
            avatar.to_string(),
        );
        self.write(move |tx| {
            tx.execute(
                "INSERT INTO web_profiles(discord_id, username, display_name, avatar, updated_at) VALUES (?,?,?,?,?) \
                 ON CONFLICT(discord_id) DO UPDATE SET username=excluded.username, \
                 display_name=excluded.display_name, avatar=excluded.avatar, updated_at=excluded.updated_at",
                rusqlite::params![id, u, d, a, now],
            )?;
            Ok(())
        })
        .await
    }

    /// 记一次网页登录的设备指纹（同账号同设备只留一行，刷新时间和 IP）。防滥用用途。
    pub async fn record_device_login(
        &self,
        discord_id: &str,
        device_id: &str,
        ip: &str,
        now: f64,
    ) -> Result<()> {
        let (d, dev, ip) = (
            discord_id.to_string(),
            device_id.to_string(),
            ip.to_string(),
        );
        self.write(move |tx| {
            tx.execute(
                "INSERT INTO device_logins(discord_id, device_id, ip, ts, first_ts) VALUES (?1,?2,?3,?4,?4) \
                 ON CONFLICT(discord_id, device_id) DO UPDATE SET ip=excluded.ip, ts=excluded.ts, \
                 first_ts=COALESCE(device_logins.first_ts, device_logins.ts)",
                rusqlite::params![d, dev, ip, now],
            )?;
            Ok(())
        })
        .await
    }

    /// 记一次网页登录（永久保留，不随 30 天清理）：第一次登录时刻不变，刷新最近一次的时刻、指纹状态和 IP。
    /// `fp_status`：ok（服务端认得的指纹）/ none（没交指纹）/ unknown（交了服务端不认得的指纹 = 疑似伪造）。
    pub async fn record_web_login(
        &self,
        discord_id: &str,
        fp_status: &str,
        ip: &str,
        now: f64,
    ) -> Result<()> {
        let (d, fp, ip) = (
            discord_id.to_string(),
            fp_status.to_string(),
            ip.to_string(),
        );
        self.write(move |tx| {
            tx.execute(
                "INSERT INTO web_logins(discord_id, first_ts, last_ts, fp, ip) VALUES (?1,?2,?2,?3,?4) \
                 ON CONFLICT(discord_id) DO UPDATE SET last_ts=excluded.last_ts, fp=excluded.fp, ip=excluded.ip",
                rusqlite::params![d, now, fp, ip],
            )?;
            Ok(())
        })
        .await
    }

    /// 服务端有没有这个设备指纹的明细（登录时判断 `dev` 是不是 /public/device-fp 算出来的）。
    pub async fn has_device_fingerprint(&self, device_id: &str) -> Result<bool> {
        let dev = device_id.to_string();
        self.read(move |c| {
            Ok(c.query_row(
                "SELECT 1 FROM device_fingerprints WHERE device_id=?",
                [&dev],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
        })
        .await
    }

    /// 这个 Discord 账号在 `since` 之后有没有在网页登录过（「每位成员到网页登录一次」开关用）。
    /// 读永久保留的 web_logins，不读 30 天就清掉的 device_logins（否则 30 天后所有人又会被拦）。
    pub async fn has_web_login_since(&self, discord_id: &str, since: f64) -> Result<bool> {
        let d = discord_id.to_string();
        self.read(move |c| {
            Ok(c.query_row(
                "SELECT 1 FROM web_logins WHERE discord_id=? AND last_ts>=? LIMIT 1",
                rusqlite::params![d, since],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
        })
        .await
    }

    /// 存 / 更新一个设备指纹的可读明细（按 device_id 去重）。防滥用用途。
    pub async fn upsert_device_fingerprint(
        &self,
        device_id: &str,
        data: &str,
        now: f64,
    ) -> Result<()> {
        let (dev, data) = (device_id.to_string(), data.to_string());
        self.write(move |tx| {
            tx.execute(
                "INSERT INTO device_fingerprints(device_id, data, ts) VALUES (?,?,?) \
                 ON CONFLICT(device_id) DO UPDATE SET data=excluded.data, ts=excluded.ts",
                rusqlite::params![dev, data, now],
            )?;
            Ok(())
        })
        .await
    }

    /// 记一次网页登录的浏览器标记（`owl_bid` Cookie；同账号同浏览器只留一行，刷新时间和 IP）。防滥用用途。
    pub async fn record_browser_login(
        &self,
        discord_id: &str,
        browser_id: &str,
        ip: &str,
        now: f64,
    ) -> Result<()> {
        let (d, b, ip) = (
            discord_id.to_string(),
            browser_id.to_string(),
            ip.to_string(),
        );
        self.write(move |tx| {
            tx.execute(
                "INSERT INTO browser_logins(discord_id, browser_id, ip, ts, first_ts) VALUES (?1,?2,?3,?4,?4) \
                 ON CONFLICT(discord_id, browser_id) DO UPDATE SET ip=excluded.ip, ts=excluded.ts, \
                 first_ts=COALESCE(browser_logins.first_ts, browser_logins.ts)",
                rusqlite::params![d, b, ip, now],
            )?;
            Ok(())
        })
        .await
    }

    /// 占用名额的人数：持有未过期 Key 的已注册用户。
    /// Key 过期或被删则名额释放；被站长停用（暂停）的成员仍占名额。
    pub async fn registered_member_count(&self, now: f64) -> Result<i64> {
        self.read(move |c| {
            c.query_row(
                "SELECT COUNT(*) FROM discord_registrations r JOIN api_keys k ON k.id=r.key_id \
                 WHERE k.expires_at IS NULL OR k.expires_at > ?",
                [now],
                |r| value::i64_or(r, 0, 0),
            )
        })
        .await
    }
}
