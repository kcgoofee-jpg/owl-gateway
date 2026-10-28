//! One read transaction for every input to the anti-abuse rules. No scoring or enforcement here.
use crate::{Db, Result};
use gate_core::abuse::{RequestEvent, ScanSnapshot, ScanWindow};
use rusqlite::{Connection, Row, ToSql};

fn rows<T>(
    c: &Connection,
    sql: &str,
    params: &[&dyn ToSql],
    map: impl FnMut(&Row<'_>) -> rusqlite::Result<T>,
) -> rusqlite::Result<Vec<T>> {
    c.prepare(sql)?.query_map(params, map)?.collect()
}

impl Db {
    pub async fn abuse_scan_snapshot(&self, window: ScanWindow) -> Result<ScanSnapshot> {
        self.read(move|connection|{
            let c=connection.unchecked_transaction()?;
            let snapshot=ScanSnapshot {
                keys:rows(&c,"SELECT id,discord_id FROM api_keys WHERE enabled=1 AND COALESCE(discord_id,'')!=''",&[],|r|Ok((r.get(0)?,r.get(1)?)))?,
                bans:rows(&c,"SELECT discord_id,reason,COALESCE(NULLIF(display_name,''),NULLIF(username,''),'') FROM discord_bans",&[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?,
                requests:rows(&c,"SELECT key_id,ts,COALESCE(src,''),status,COALESCE(reason,'') FROM usage_log WHERE ts>=? AND key_id IS NOT NULL ORDER BY ts",&[&window.behavior_since],|r|Ok((r.get(0)?,RequestEvent{ts:r.get(1)?,ip:r.get(2)?,ok:r.get::<_,String>(3)?=="ok",member_quota:r.get::<_,String>(4)?=="member_quota"})))?,
                devices:rows(&c,"SELECT device_id,data FROM device_fingerprints",&[],|r|Ok((r.get(0)?,r.get(1)?)))?,
                device_logins:rows(&c,"SELECT discord_id,device_id FROM device_logins WHERE device_id!=''",&[],|r|Ok((r.get(0)?,r.get(1)?)))?,
                browser_logins:rows(&c,"SELECT discord_id,browser_id FROM browser_logins",&[],|r|Ok((r.get(0)?,r.get(1)?)))?,
                incomplete_logins:rows(&c,"SELECT discord_id,fp,last_ts FROM web_logins WHERE fp IN ('none','unknown') AND last_ts>=?",&[&window.fingerprint_since],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?,
                registrations:rows(&c,"SELECT discord_id,created_at,guild_joined_at,COALESCE(member_flags,0),COALESCE(avatar,''),COALESCE(username,'') FROM discord_registrations WHERE created_at>=?",&[&window.metadata_since],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?)))?,
                login_networks:rows(&c,"SELECT discord_id,ip FROM device_logins WHERE ip!='' UNION ALL SELECT discord_id,ip FROM browser_logins WHERE ip!='' UNION ALL SELECT discord_id,ip FROM web_logins WHERE ip!=''",&[],|r|Ok((r.get(0)?,r.get(1)?)))?,
                references:rows(&c,"SELECT sha256,dhash,key_id,ts FROM ref_images WHERE ts>=? ORDER BY ts",&[&window.linkage_since],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?,
                connections:rows(&c,"SELECT type,ext_id,discord_id FROM discord_user_connections",&[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?,
                guilds:rows(&c,"SELECT discord_id,guild_id,name FROM discord_user_guilds",&[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?,
                batteries:rows(&c,"SELECT ts,discord_id,level,charging FROM device_battery WHERE ts>=? ORDER BY ts",&[&window.linkage_since],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get::<_,i64>(3)?!=0)))?,
                origins:rows(&c,"SELECT key_id,origin,COUNT(*),MIN(ts),MAX(ts) FROM usage_log WHERE ts>=? AND key_id IS NOT NULL AND COALESCE(origin,'')!='' GROUP BY key_id,origin",&[&window.linkage_since],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?)))?,
            };
            c.commit()?;
            Ok(snapshot)
        }).await
    }
}
