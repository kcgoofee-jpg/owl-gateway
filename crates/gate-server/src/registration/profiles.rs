//! 成员资料：读公开资料、补全历史资料、记录进服时间与成员标记位。

use super::*;

impl RegistrationService {
    /// 用机器人读一个 Discord 账号的公开资料（`GET /users/{id}`）。网络错误、超时、非 200、
    /// 响应不是 JSON 对象都返回 None；`secs` 是整个请求的超时。
    pub async fn fetch_user(&self, discord_id: &str, secs: u64) -> Option<DiscordUser> {
        let path = format!("/users/{discord_id}");
        match self
            .send(Method::GET, &path, &self.bot_auth(), secs, None)
            .await
        {
            Ok(r) if r.status == 200 => r
                .json()
                .filter(Value::is_object)
                .map(|v| DiscordUser::from_json(&v)),
            _ => None,
        }
    }

    /// 给还没有 Discord 用户名 / 头像的登记补全（用机器人读取公开资料）；每次最多处理几条，避免触发限流。
    pub async fn backfill_profiles(&self, limit: i64) -> Result<usize, DbError> {
        let mut done = 0;
        for (discord_id, key_id) in self.db.profile_backfill_candidates(limit).await? {
            let Some(u) = self.fetch_user(&discord_id, 10).await else {
                continue;
            };
            let profile = RegistrationProfile {
                username: Some(clip(&u.username).unwrap_or_default()),
                display_name: clip(&u.global_name),
                avatar: clip(&u.avatar),
            };
            self.db
                .apply_profile_backfill(&discord_id, key_id, &profile, &member_label(&u))
                .await?;
            done += 1;
        }
        Ok(done)
    }

    /// 记下成员的进服时间 / 标记位（只更新已有的领取记录）。尽力而为：写库失败只记日志，不影响登录和退群核对。
    pub async fn record_member_join(&self, discord_id: &str, join: MemberJoin) {
        if let Err(e) = self
            .db
            .record_member_join(discord_id, join.joined_at, join.flags)
            .await
        {
            tracing::warn!("[member-join] record failed: {e}");
        }
    }

    /// 领取记录里的资料：没有记录只有 `{"id"}`。
    pub async fn registration_profile(&self, discord_id: &str) -> Result<Value, DbError> {
        Ok(match self.db.discord_profile_row(discord_id).await? {
            None => json!({"id": discord_id}),
            Some((username, display_name, avatar)) => {
                json!({"id": discord_id, "username": username, "display_name": display_name, "avatar": avatar})
            }
        })
    }
}
