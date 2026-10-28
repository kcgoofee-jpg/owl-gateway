//! 小号 / 共享 Key 的待审信号（`abuse_signals`）：检测代码按 (kind, subject) upsert，站长在后台「待审」确认或忽略。
//!
//! 规则：同一 (kind, subject) 只有一行，再次命中只刷新 key_ids / score / detail / last_ts；
//! 已忽略的信号分数比原来高出 ≥ 10 才重新打开；已确认的不动状态。

use crate::{Db, Result, value};
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};
use std::collections::HashMap;

/// 和某个账号用过同一个浏览器 / 同一台设备网页登录的另一个账号（[`Db::login_peers`]）。
#[derive(Debug, Clone, PartialEq)]
pub struct LoginPeer {
    pub discord_id: String,
    /// `browser` / `device`
    pub via: &'static str,
    /// 设备指纹明细 JSON（`device` 才有；没存过为空串）
    pub device_data: String,
}

pub use gate_core::abuse::banned_link::BanInfo;

/// 待审卡片上每个账号的「登录时间」从哪儿取。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LoginScope {
    /// 只算和其他涉及账号共用的浏览器
    Browser,
    /// 只算和其他涉及账号共用的设备指纹
    Device,
    /// 共用的浏览器和设备都算
    Both,
    /// 账号的网页登录（不限设备）
    Account,
}

/// 信号状态。
pub const ABUSE_STATUSES: [&str; 3] = ["open", "dismissed", "confirmed"];
/// 已忽略的信号要比原分数高出这么多才重新进待审。
pub const ABUSE_REOPEN_DELTA: f64 = 10.0;

/// Owner review is metadata, never an automatic ban.
pub const REVIEW_TAG: &str = "风险线索";
#[derive(Clone, Debug)]
pub struct AbuseFilter {
    pub status: Option<String>,
    pub member: Option<String>,
    pub limit: i64,
    pub offset: i64,
}
impl Default for AbuseFilter {
    fn default() -> Self {
        Self {
            status: None,
            member: None,
            limit: 200,
            offset: 0,
        }
    }
}
#[derive(Default, serde::Serialize)]
pub struct AbuseCounts {
    pub open: i64,
    pub confirmed: i64,
    pub dismissed: i64,
    pub expired: i64,
    pub total: i64,
}
#[derive(Clone, Copy, Debug, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReviewAction {
    Dismiss,
    Confirm,
    Reopen,
}
impl ReviewAction {
    pub fn status(self) -> &'static str {
        match self {
            Self::Dismiss => "dismissed",
            Self::Confirm => "confirmed",
            Self::Reopen => "open",
        }
    }
}
pub struct ReviewedSignal {
    pub kind: String,
    pub key_ids: Vec<i64>,
    pub tagged: usize,
}
const MEMBER_MATCH: &str = "(?2 IS NULL OR instr(','||s.subject||',',','||?2||',')>0 OR EXISTS(SELECT 1 FROM api_keys k WHERE k.discord_id=?2 AND (s.subject='key:'||k.id OR instr(','||replace(s.key_ids,' ','')||',',','||k.id||',')>0)))";

impl Db {
    /// 新增或刷新一条信号；返回行 id。
    pub async fn upsert_abuse_signal(
        &self,
        kind: &str,
        subject: &str,
        key_ids: &str,
        score: f64,
        detail_json: &str,
        now: f64,
    ) -> Result<i64> {
        let (kind, subject, key_ids, detail) = (
            kind.to_string(),
            subject.to_string(),
            key_ids.to_string(),
            detail_json.to_string(),
        );
        let score = score.clamp(0.0, 100.0);
        self.write(move |tx| {
            tx.query_row(
                "INSERT INTO abuse_signals(kind, subject, key_ids, score, detail, status, first_ts, last_ts) \
                 VALUES (?1,?2,?3,?4,?5,'open',?6,?6) \
                 ON CONFLICT(kind, subject) DO UPDATE SET \
                 status = CASE WHEN abuse_signals.status='expired' THEN 'open' \
                   WHEN abuse_signals.status='dismissed' \
                   AND excluded.score >= abuse_signals.score + ?7 THEN 'open' ELSE abuse_signals.status END, \
                 key_ids=excluded.key_ids, score=excluded.score, detail=excluded.detail, last_ts=excluded.last_ts \
                 RETURNING id",
                params![kind, subject, key_ids, score, detail, now, ABUSE_REOPEN_DELTA],
                |r| r.get(0),
            )
        })
        .await
    }

    /// Filter before pagination, including stable subjects whose credential has rotated.
    pub async fn list_abuse_signals(&self, filter: &AbuseFilter) -> Result<Vec<Value>> {
        let f = filter.clone();
        self.read(move|c|{
            let sql=format!("SELECT s.id,s.kind,s.subject,s.key_ids,s.score,s.detail,s.status,s.first_ts,s.last_ts FROM abuse_signals s WHERE (?1 IS NULL OR s.status=?1) AND {MEMBER_MATCH} ORDER BY s.score DESC,s.last_ts DESC,s.id DESC LIMIT ?3 OFFSET ?4");
            c.prepare(&sql)?.query_map(params![f.status,f.member,f.limit,f.offset],value::row_json)?.collect()
        }).await
    }
    pub async fn abuse_signal_counts(&self, member: Option<&str>) -> Result<AbuseCounts> {
        let member = member.map(str::to_owned);
        self.read(move|c|{
            let mut counts=AbuseCounts::default();
            let sql=format!("SELECT s.status,count(*) FROM abuse_signals s WHERE {MEMBER_MATCH} GROUP BY s.status");
            let mut stmt=c.prepare(&sql)?;
            for row in stmt.query_map(params![Option::<String>::None,member],|r|Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?)))? {
                let (status,n)=row?;counts.total+=n;
                match status.as_str(){"open"=>counts.open=n,"confirmed"=>counts.confirmed=n,"dismissed"=>counts.dismissed=n,"expired"=>counts.expired=n,_=>{}}
            }
            Ok(counts)
        }).await
    }
    /// Status and tags commit together; missing/deleted credentials never receive a tag.
    pub async fn review_abuse_signal(
        &self,
        id: i64,
        action: ReviewAction,
        actor: &str,
        now: f64,
    ) -> Result<Option<ReviewedSignal>> {
        let actor = actor.to_owned();
        self.write(move|tx|{
            let row=tx.query_row("SELECT kind,key_ids FROM abuse_signals WHERE id=?",[id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?))).optional()?;
            let Some((kind,raw))=row else{return Ok(None)};
            let mut key_ids=Vec::new();
            for id in raw.split(',').filter_map(|part|part.trim().parse::<i64>().ok()).filter(|id|*id>0){if !key_ids.contains(&id){key_ids.push(id)}}
            tx.execute("UPDATE abuse_signals SET status=? WHERE id=?",params![action.status(),id])?;
            let mut tagged=0;
            if action==ReviewAction::Confirm {
                for key in &key_ids {
                    tagged+=tx.execute("INSERT INTO key_tags(key_id,tag,note,by,ts) SELECT id,?2,?3,?4,?5 FROM api_keys WHERE id=?1 ON CONFLICT(key_id,tag) DO UPDATE SET note=excluded.note,by=excluded.by,ts=excluded.ts",params![key,REVIEW_TAG,kind,actor,now])?;
                }
            }
            Ok(Some(ReviewedSignal{kind,key_ids,tagged}))
        }).await
    }

    pub async fn get_abuse_signal(&self, id: i64) -> Result<Option<Value>> {
        self.read(move |c| {
            c.query_row(
                "SELECT id, kind, subject, key_ids, score, detail, status, first_ts, last_ts \
                 FROM abuse_signals WHERE id=?",
                [id],
                value::row_json,
            )
            .optional()
        })
        .await
    }

    /// 这一轮扫描没再出现、站长也还没处理的待审信号（`last_ts` 早于本轮开始）改成 `expired`：
    /// 不再出现在「待审」里，但记录留着（站长 10-22：后台记录一律留存）。下次又扫到会被 upsert 重新打开。
    pub async fn expire_stale_open_abuse_signals(&self, before: f64) -> Result<usize> {
        self.write(move |tx| {
            tx.execute(
                "UPDATE abuse_signals SET status='expired' WHERE status='open' AND last_ts<?",
                [before],
            )
        })
        .await
    }

    /// 改状态；没有这条返回 false。
    pub async fn set_abuse_status(&self, id: i64, status: &str) -> Result<bool> {
        let status = status.to_string();
        self.write(move |tx| {
            Ok(tx.execute(
                "UPDATE abuse_signals SET status=? WHERE id=?",
                params![status, id],
            )? > 0)
        })
        .await
    }

    /// 信号里各 Key 的名字和 Discord 名（按传入顺序）。Key 已删的取删除前留下的归属（`deleted` = true）；
    /// Discord 名先看领 Key 时登记的，再看网页登录时给的。
    pub async fn abuse_key_accounts(&self, ids: Vec<i64>) -> Result<Vec<Value>> {
        self.read(move |c| {
            let mut live = c.prepare(
                "SELECT k.name, COALESCE(k.discord_id, r.discord_id), \
                 COALESCE(NULLIF(r.display_name,''), NULLIF(r.username,'')) \
                 FROM api_keys k LEFT JOIN discord_registrations r ON r.key_id=k.id WHERE k.id=?",
            )?;
            let mut gone = c.prepare(
                "SELECT NULLIF(name,''), discord_id FROM deleted_key_owners WHERE key_id=?",
            )?;
            let mut web = c.prepare(
                "SELECT COALESCE(NULLIF(display_name,''), NULLIF(username,'')) \
                 FROM web_profiles WHERE discord_id=?",
            )?;
            let mut out = Vec::with_capacity(ids.len());
            for id in ids {
                type Row = (Option<String>, Option<String>, Option<String>);
                let row: Option<Row> = live
                    .query_row([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                    .optional()?;
                let deleted = row.is_none();
                let (name, did, mut dname) = match row {
                    Some(r) => r,
                    None => gone
                        .query_row([id], |r| Ok((r.get(0)?, r.get(1)?, None)))
                        .optional()?
                        .unwrap_or_default(),
                };
                if dname.is_none()
                    && let Some(d) = &did
                {
                    dname = web
                        .query_row([d], |r| r.get::<_, Option<String>>(0))
                        .optional()?
                        .flatten();
                }
                out.push(json!({
                    "key_id": id, "name": name, "discord_id": did,
                    "discord_name": dname, "deleted": deleted,
                }));
            }
            Ok(out)
        })
        .await
    }

    /// (kind, subject) 那条信号的状态；没有这条返回 None。
    pub async fn abuse_signal_status(&self, kind: &str, subject: &str) -> Result<Option<String>> {
        let (kind, subject) = (kind.to_string(), subject.to_string());
        self.read(move |c| {
            c.query_row(
                "SELECT status FROM abuse_signals WHERE kind=? AND subject=?",
                params![kind, subject],
                |r| r.get(0),
            )
            .optional()
        })
        .await
    }

    /// 还在待审的信号改成已确认（涉及的账号已经全部封禁时用：信号不再带来新信息）。改了返回 true。
    pub async fn confirm_open_abuse_signal(&self, id: i64) -> Result<bool> {
        self.write(move |tx| {
            Ok(tx.execute(
                "UPDATE abuse_signals SET status='confirmed' WHERE id=? AND status='open'",
                [id],
            )? > 0)
        })
        .await
    }

    /// 和这个账号在同一个浏览器（browser_logins）或同一个设备指纹（device_logins）下网页登录过的其他账号。
    /// 同一个人可能同时出现两行（浏览器和设备各一行）。
    pub async fn login_peers(&self, discord_id: &str) -> Result<Vec<LoginPeer>> {
        let d = discord_id.to_string();
        self.read(move |c| {
            let mut out = Vec::new();
            let mut st = c.prepare(
                "SELECT DISTINCT b2.discord_id FROM browser_logins b1 \
                 JOIN browser_logins b2 ON b2.browser_id=b1.browser_id AND b2.discord_id!=b1.discord_id \
                 WHERE b1.discord_id=? AND b1.browser_id!=''",
            )?;
            for id in st.query_map([&d], |r| r.get::<_, String>(0))? {
                out.push(LoginPeer {
                    discord_id: id?,
                    via: "browser",
                    device_data: String::new(),
                });
            }
            let mut st = c.prepare(
                "SELECT d2.discord_id, COALESCE(f.data,'') FROM device_logins d1 \
                 JOIN device_logins d2 ON d2.device_id=d1.device_id AND d2.discord_id!=d1.discord_id \
                 LEFT JOIN device_fingerprints f ON f.device_id=d1.device_id \
                 WHERE d1.discord_id=? AND d1.device_id!=''",
            )?;
            for row in st.query_map([&d], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
                let (id, data) = row?;
                out.push(LoginPeer {
                    discord_id: id,
                    via: "device",
                    device_data: data,
                });
            }
            Ok(out)
        })
        .await
    }

    /// 这些账号里在封禁名单上的：Discord ID → 名字 / 理由。
    pub async fn ban_infos(&self, ids: Vec<String>) -> Result<HashMap<String, BanInfo>> {
        self.read(move |c| {
            let mut st = c.prepare(
                "SELECT reason, COALESCE(NULLIF(display_name,''), NULLIF(username,''), '') \
                 FROM discord_bans WHERE discord_id=?",
            )?;
            let mut out = HashMap::new();
            for id in ids {
                if let Some((reason, name)) = st
                    .query_row([&id], |r| {
                        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                    })
                    .optional()?
                {
                    out.insert(id, BanInfo { name, reason });
                }
            }
            Ok(out)
        })
        .await
    }

    /// 待审卡片上的账号列表（按传入顺序）：名字 / 头像、Key（有的话）、封禁状态、首次 / 最近登录时间。
    /// 名字先看领 Key 时登记的，再看网页登录时给的，最后看封禁名单里记的。不含 IP。
    /// 登录时间按 `scope`：共用的浏览器 / 设备只算和 `ids` 里其他账号重合的那几个。
    pub async fn abuse_members(&self, ids: Vec<String>, scope: LoginScope) -> Result<Vec<Value>> {
        self.read(move |c| {
            let all = ids.join(",");
            let mut prof = c.prepare(
                "SELECT username, display_name, avatar FROM ( \
                 SELECT 1 AS o, COALESCE(username,'') AS username, COALESCE(display_name,'') AS display_name, \
                   COALESCE(avatar,'') AS avatar FROM discord_registrations WHERE discord_id=?1 \
                 UNION ALL SELECT 2, username, display_name, avatar FROM web_profiles WHERE discord_id=?1 \
                 UNION ALL SELECT 3, username, display_name, avatar FROM discord_bans WHERE discord_id=?1) \
                 WHERE username!='' OR display_name!='' ORDER BY o LIMIT 1",
            )?;
            let mut key = c.prepare(
                "SELECT k.id, k.name FROM api_keys k WHERE k.id=(SELECT key_id FROM discord_registrations WHERE discord_id=?1) \
                 UNION ALL SELECT id, name FROM (SELECT id, name FROM api_keys WHERE discord_id=?1 ORDER BY id DESC LIMIT 1) \
                 LIMIT 1",
            )?;
            let mut ban = c.prepare("SELECT reason FROM discord_bans WHERE discord_id=?")?;
            // 和其他涉及账号共用的浏览器 / 设备上的首次、最近登录（老行没有 first_ts，退回 ts）
            let others = "instr(','||?2||',', ','||discord_id||',')>0 AND discord_id!=?1";
            let mut browser = c.prepare(&format!(
                "SELECT MIN(COALESCE(first_ts, ts)), MAX(ts) FROM browser_logins WHERE discord_id=?1 \
                 AND browser_id IN (SELECT browser_id FROM browser_logins WHERE {others})"
            ))?;
            let mut device = c.prepare(&format!(
                "SELECT MIN(COALESCE(first_ts, ts)), MAX(ts) FROM device_logins WHERE discord_id=?1 \
                 AND device_id IN (SELECT device_id FROM device_logins WHERE {others})"
            ))?;
            let mut account =
                c.prepare("SELECT first_ts, last_ts FROM web_logins WHERE discord_id=?1")?;
            let span = |st: &mut rusqlite::Statement<'_>, d: &str, with_all: bool| {
                let row = |r: &rusqlite::Row<'_>| {
                    Ok((r.get::<_, Option<f64>>(0)?, r.get::<_, Option<f64>>(1)?))
                };
                if with_all {
                    st.query_row(params![d, all], row)
                } else {
                    st.query_row([d], row)
                }
                .optional()
                .map(|o| o.unwrap_or((None, None)))
            };
            let mut out = Vec::with_capacity(ids.len());
            for d in &ids {
                let (username, display_name, avatar) = prof
                    .query_row([d], |r| {
                        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
                    })
                    .optional()?
                    .unwrap_or_default();
                let k: Option<(i64, String)> = key
                    .query_row([d], |r| Ok((r.get(0)?, r.get(1)?)))
                    .optional()?;
                let ban_reason: Option<String> = ban.query_row([d], |r| r.get(0)).optional()?;
                let mut spans = Vec::new();
                if matches!(scope, LoginScope::Browser | LoginScope::Both) {
                    spans.push(span(&mut browser, d, true)?);
                }
                if matches!(scope, LoginScope::Device | LoginScope::Both) {
                    spans.push(span(&mut device, d, true)?);
                }
                if scope == LoginScope::Account {
                    spans.push(span(&mut account, d, false)?);
                }
                let first = spans.iter().filter_map(|s| s.0).reduce(f64::min);
                let last = spans.iter().filter_map(|s| s.1).reduce(f64::max);
                out.push(json!({
                    "discord_id": d,
                    "username": username,
                    "display_name": display_name,
                    "avatar": avatar,
                    "key_id": k.as_ref().map(|k| k.0),
                    "key_name": k.map(|k| k.1),
                    "banned": ban_reason.is_some(),
                    "ban_reason": ban_reason,
                    "first_login": first,
                    "last_login": last,
                }));
            }
            Ok(out)
        })
        .await
    }
}
