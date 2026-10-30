//! 身份组：给予 / 摘除、全量同步、退群清理、验证授权里的身份组检查。

use super::*;

impl RegistrationService {
    /// 给 / 摘「已领 Key」身份组。失败只返回 false，不影响注册或撤销。
    pub async fn set_role(&self, discord_id: &str, grant: bool) -> bool {
        if self.cfg.member_role_id.is_empty() {
            return false;
        }
        let path = format!(
            "/guilds/{}/members/{discord_id}/roles/{}",
            self.cfg.guild_id, self.cfg.member_role_id
        );
        let reason = format!("NAI Gate key {}", if grant { "issued" } else { "ended" });
        let req = self
            .http
            .request(
                if grant { Method::PUT } else { Method::DELETE },
                self.url(&path),
            )
            .header("Authorization", self.bot_auth())
            .header("X-Audit-Log-Reason", reason)
            .timeout(Duration::from_secs(10));
        match req.send().await {
            // 404: 成员已离开服务器，也算完成
            Ok(r) => matches!(r.status().as_u16(), 200 | 204 | 404),
            Err(_) => false,
        }
    }

    /// 摘身份组：先登记待办，成功后再划掉；失败（限流、网络）会由后台同步重试，不会永久残留。
    pub async fn release_role(&self, discord_id: &str) -> Result<(), DbError> {
        if self.cfg.member_role_id.is_empty() {
            return Ok(());
        }
        self.db
            .queue_role_removal(discord_id, crate::time::now())
            .await?;
        if self.set_role(discord_id, false).await {
            self.db.finish_role_removal(discord_id).await?;
        }
        Ok(())
    }

    /// 后台同步：Key 已过期 / 被停用 / 被删除的成员摘掉身份组，并重试之前失败的摘除。返回本次摘掉的人数。
    ///
    /// 每轮最多处理 [`SYNC_ROLES_PER_RUN`] 个（维护循环每 5 分钟一轮），和 `backfill_profiles(5)` 一样不制造突发请求：
    /// 全站重置一次堆起几十个待办时，分几轮慢慢摘，剩下的留到下一轮（待办表不丢）。
    pub async fn sync_roles(&self) -> Result<usize, DbError> {
        if self.cfg.member_role_id.is_empty() {
            return Ok(0);
        }
        self.db
            .queue_expired_role_removals(crate::time::now())
            .await?;
        let mut done = 0;
        for discord_id in self
            .db
            .pending_role_removals()
            .await?
            .into_iter()
            .take(SYNC_ROLES_PER_RUN)
        {
            if self.set_role(&discord_id, false).await {
                // 每摘一个就提交：不能在持有未提交写事务时去等下一个 Discord 请求
                self.db.finish_role_removal(&discord_id).await?;
                done += 1;
            }
        }
        Ok(done)
    }

    /// 退群回收：每个领了 Key 的人每天核对一次是否还在本服务器，不在就删 Key、放名额（站长 2026-10-10 决定直接回收）。
    ///
    /// 维护循环每 5 分钟调一次、每次只查 1 人 → 约 49 次读取 / 天，均匀分散，不发私信（申诉期间不制造突发请求）。
    /// 只有 Discord 明确回复「不是成员」（404 + code 10007）才回收；网络错误、限流、机器人不在服务器（10004）
    /// 或任何其他情况一律跳过、下轮再查，宁可漏收也不误删。返回本次回收人数。
    ///
    /// 邀请 Key 只由站长手动删（与每日清空 / 全站重置同一口径）：持有人退群时不删 Key，只写一条操作日志
    /// 「退群（邀请 Key 保留）」提醒站长，并把下次核对推到 30 天后（每月最多提醒一条）。
    pub async fn sweep_departed(&self, now: f64, batch: i64) -> Result<usize, DbError> {
        if !self.flag(&site_flags::MEMBER_SWEEP).await? {
            return Ok(0);
        }
        let mut removed = 0;
        for cand in self.db.departed_candidates(now, batch).await? {
            let discord_id = cand.discord_id;
            let path = format!("/guilds/{}/members/{discord_id}", self.cfg.guild_id);
            // 查不清（网络错误 / 404 的响应体读不出 code）都当作「没有响应」
            let answer: Option<(u16, Option<Value>)> = match self
                .send(Method::GET, &path, &self.bot_auth(), 10, None)
                .await
            {
                Ok(r) if r.status == 404 => r
                    .json()
                    .filter(Value::is_object)
                    .map(|v| (404, v.get("code").cloned())),
                // 这里查的是机器人所在的服务器（本站自己的服），不是领 Key 门槛要求的服：它的进服时间
                // 不能当「进服多久」用（大家都是最近才进的本站服，会被全当成新人），所以不记。
                // 进服时间只在网页登录核验身份组时，从门槛服务器的成员信息里记。
                Ok(r) if r.status == 200 => Some((200, None)),
                Ok(r) => Some((r.status, None)),
                Err(_) => None,
            };
            if let Some((404, Some(code))) = &answer
                && values::number(code) == Some(10007.0)
            {
                let label = self.registration_profile(&discord_id).await?;
                let who = ["display_name", "username"]
                    .iter()
                    .find_map(|k| label.get(k).filter(|v| values::enabled(v)))
                    .map(validation::text)
                    .unwrap_or_else(|| discord_id.clone());
                if self.revoke(&discord_id, false).await? {
                    log_action(
                        &self.db,
                        "系统",
                        "退群回收 Key",
                        &format!("Discord:{discord_id} {who}"),
                        "已不在服务器，Key 已删除、名额释放",
                        true,
                        now,
                    )
                    .await;
                    removed += 1;
                }
                continue;
            }
            // 在服务器 → 明天再查；没查清（限流 / 网络 / 其他错误）→ 1 小时后重查，不能一直卡在队首挡住别人
            let checked = if matches!(answer, Some((200, _))) {
                now
            } else {
                now - 86400.0 + 3600.0
            };
            self.db.set_member_checked(&discord_id, checked).await?;
        }
        Ok(removed)
    }

    /// 用成员自己的授权查他在门槛服务器里的身份组；有就记入 `role_verifications`。
    /// 门槛可以有多条（后台「领 Key」的主门槛 + 额外身份组，满足任意一条就算通过），可以分属不同的服务器：
    /// 同一个服务器只查一次，里面哪几条身份组命中就记哪几条。没配 = 不限定。
    ///
    /// 结果：有命中 = `Verified`；所有服务器都明确回答「不在 / 没有」= `Missing`；
    /// 没命中、但有服务器故障 / 限流 / 网络错误 = `Unavailable`（不能说成「你没有身份组」）。
    pub async fn check_role(&self, user_id: &str, bearer: &str) -> Result<RoleCheck, RegErr> {
        let cfg = self.settings().await?;
        let rules = cfg.role_rules();
        if rules.is_empty() {
            return Ok(RoleCheck::NotRequired);
        }
        self.db.clear_role_verifications(user_id).await?;
        let mut guilds: Vec<&str> = Vec::new();
        for r in &rules {
            if !guilds.contains(&r.guild.as_str()) {
                guilds.push(&r.guild);
            }
        }
        let (mut verified, mut unavailable) = (false, false);
        let mut join: Option<MemberJoin> = None;
        for guild in guilds {
            let path = format!("/users/@me/guilds/{guild}/member");
            let Ok(r) = self.send(Method::GET, &path, bearer, 12, None).await else {
                unavailable = true;
                continue;
            };
            match r.status {
                200 => {}
                // 10007 Unknown Member：账号不在这个服务器，这个服务器里的门槛都不满足
                404 => continue,
                _ => {
                    unavailable = true;
                    continue;
                }
            }
            let Some(member) = r.json() else {
                unavailable = true;
                continue;
            };
            let this = MemberJoin::from_member(&member);
            join = Some(join.map_or(this, |j| j.merge(this)));
            let roles: Vec<&str> = member
                .get("roles")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            for rule in rules.iter().filter(|r| r.guild == guild) {
                if roles.contains(&rule.role.as_str()) {
                    self.db
                        .upsert_role_verification(
                            user_id,
                            &rule.guild,
                            &rule.role,
                            crate::time::now(),
                        )
                        .await?;
                    verified = true;
                }
            }
        }
        if let Some(j) = join {
            self.record_member_join(user_id, j).await;
        }
        Ok(if verified {
            RoleCheck::Verified
        } else if unavailable {
            RoleCheck::Unavailable
        } else {
            RoleCheck::Missing
        })
    }
}
