//! 站长手动私信成员（奶妹机器人发）。
//!
//! 2026-10-10 奶妹应用被 Discord 判为 Application Growth Disabled（680009），触发信号之一是「给每个新用户自动私信发 Key」。
//! 所以这里只允许站长在后台逐个手动发、小批量：每小时 / 每天有上限，消息不带任何提及。
//! 不要接到任何自动流程（发 Key、封禁、告警）上；要群发请走频道公告。

use super::*;

/// 每小时最多私信几条。
pub const DM_PER_HOUR: usize = 10;
/// 每 24 小时最多私信几条。
pub const DM_PER_DAY: usize = 30;
/// 一条私信最长多少字（Discord 上限 2000）。
pub const DM_MAX_CHARS: usize = 1500;

impl RegistrationService {
    /// 先占一个名额再发：失败也算用掉（Discord 那边同样计了一次请求）。
    async fn take_dm_slot(&self, now: f64) -> Result<(), String> {
        match self
            .db
            .reserve_admin_dm(now, DM_PER_HOUR as i64, DM_PER_DAY as i64)
            .await
        {
            Ok(0) => Ok(()),
            Ok(1) => Err(format!("私信太频繁：每小时最多 {DM_PER_HOUR} 条，稍后再发")),
            Ok(_) => Err(format!("今天私信已达上限 {DM_PER_DAY} 条（滚动 24 小时）")),
            Err(_) => Err("无法读取私信限频记录，本次未发送".to_string()),
        }
    }

    /// 打开与成员的私信频道并发一条纯文本。返回 Err 时是给站长看的中文原因。
    pub async fn send_dm(&self, discord_id: &str, text: &str) -> Result<(), String> {
        self.take_dm_slot(crate::time::now()).await?;
        let auth = self.bot_auth();
        let open = self
            .send(
                Method::POST,
                "/users/@me/channels",
                &auth,
                12,
                Some(&json!({"recipient_id": discord_id})),
            )
            .await
            .map_err(|_| "连不上 Discord，未发送私信".to_string())?;
        let channel = open
            .json()
            .filter(|_| open.status < 400)
            .and_then(|v| v["id"].as_str().map(str::to_owned))
            .ok_or_else(|| discord_reason("打不开私信频道", &open))?;
        let body = json!({"content": text, "allowed_mentions": {"parse": []}});
        let sent = self
            .send(
                Method::POST,
                &format!("/channels/{channel}/messages"),
                &auth,
                12,
                Some(&body),
            )
            .await
            .map_err(|_| "发送结果未知：Discord 可能已收到；请先核实，不要立即重发".to_string())?;
        if sent.status >= 500 {
            return Err(
                "发送结果未知：Discord 返回服务器错误，可能已收到；请先核实，不要立即重发".into(),
            );
        }
        if sent.status >= 400 {
            return Err(discord_reason("发送失败", &sent));
        }
        Ok(())
    }
}

/// 50007 = 对方关了服务器成员私信或屏蔽了机器人，最常见，单独说清楚。
fn discord_reason(what: &str, r: &Reply) -> String {
    let v = r.json().unwrap_or_default();
    if v["code"].as_i64() == Some(50007) {
        return format!("{what}：对方关闭了私信或屏蔽了机器人（50007）");
    }
    let msg = v["message"].as_str().unwrap_or("");
    format!("{what}：Discord {} {}", r.status, take_chars(msg, 120))
        .trim_end()
        .to_string()
}
