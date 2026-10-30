//! 发 Key / 封禁 / 撤销 / 回收：名额检查、频次限制、邀请 Key 绑定、起号与发号。

use super::*;

impl RegistrationService {
    /// 占用名额的人数：持有未过期 Key 的已注册用户。Key 过期或被删则名额释放。
    /// 被站长停用（暂停）的成员仍占名额：否则停用即空出名额，重新启用后会超出上限。
    pub async fn count_active(&self) -> Result<i64, DbError> {
        self.db.registered_member_count(crate::time::now()).await
    }

    /// 生效的注册设置（和后台共用 `ops::registration_settings_for`，不另写一份）。
    pub async fn settings(&self) -> Result<Registration, DbError> {
        ops::registration_settings_for(&self.db, true).await
    }

    /// 在黑名单里就返回拉黑理由，不在返回 None。
    pub async fn ban_reason(&self, discord_id: &str) -> Result<Option<String>, DbError> {
        self.db.discord_ban_reason(discord_id).await
    }

    pub async fn key_row_for(&self, discord_id: &str) -> Result<Option<Key>, DbError> {
        match self.db.key_row_for_discord(discord_id).await? {
            Some(row) => Ok(Some(Key::load(&self.db, row).await?)),
            None => Ok(None),
        }
    }

    /// 名额检查：有空位先到先得，满了直接拒绝（候补排队已移除，2026-10-13）。
    pub async fn check_capacity(&self) -> Result<Registration, RegErr> {
        let cfg = self.settings().await?;
        if !cfg.open {
            return denied("领 Key 当前已关闭，开放时间看站长公告。");
        }
        if cfg.max_users == 0 || cfg.max_users - self.count_active().await? > 0 {
            return Ok(cfg);
        }
        // 只有开着闲置回收时才提它（10-17 起已关闭，写了就是误导）
        let recycle = if self.idle_days() != 0 {
            "闲置的 Key 会被自动回收，"
        } else {
            ""
        };
        denied(format!(
            "名额已满（上限 {} 人），请过段时间再试。{recycle}空出的名额先到先得。",
            cfg.max_users
        ))
    }

    /// 永久禁止该 Discord 账号领取：写入封禁表，并撤销其现有 Key 与身份组。
    pub async fn ban(
        &self,
        discord_id: &str,
        reason: &str,
        banned_by: &str,
    ) -> Result<(), DbError> {
        // 记录用户的 Discord 资料，供后台黑名单展示
        let (u, d, a) = self
            .db
            .discord_profile_row(discord_id)
            .await?
            .unwrap_or_default();
        let had_key = {
            let _g = self.lock.lock().await;
            self.db
                .ban_discord(
                    discord_id,
                    &BanInfo {
                        reason: if reason.is_empty() {
                            "站长手动封禁".into()
                        } else {
                            reason.into()
                        },
                        username: u.unwrap_or_default(),
                        display_name: d.unwrap_or_default(),
                        avatar: a.unwrap_or_default(),
                        banned_by: if banned_by.is_empty() {
                            "站长".into()
                        } else {
                            banned_by.into()
                        },
                    },
                    crate::time::now(),
                )
                .await?;
            self.revoke(discord_id, false).await?
        };
        if had_key {
            self.release_role(discord_id).await?;
        }
        Ok(())
    }

    /// 成员的 Key 已过期：自动清掉旧 Key 和记录，让他可以在有名额时重新领取（每个周期自然轮换）。
    /// 被站长停用（enabled=0）的 Key 不会被自动释放：停用等于暂停该成员，不能靠过期绕过。
    pub async fn release_if_expired(
        &self,
        discord_id: &str,
        defer_role: bool,
    ) -> Result<(), DbError> {
        if let Some(key) = self.key_row_for(discord_id).await?
            && key.enabled()
            && key.expired(crate::time::now())
        {
            self.revoke(discord_id, !defer_role).await?;
            log_action(
                &self.db,
                "系统",
                "Key 到期释放名额",
                &format!("Key #{} {}", key.id(), key.name()),
                &format!("Discord:{discord_id} 重新领取时自动释放"),
                true,
                crate::time::now(),
            )
            .await;
        }
        Ok(())
    }

    /// 删除该用户的 Key 和领取记录，名额释放，用户可重新领取。`remove_role=false` 时只登记待办，
    /// 由后台同步稍后执行，避免在锁内等 Discord。
    pub async fn revoke(&self, discord_id: &str, remove_role: bool) -> Result<bool, DbError> {
        if self
            .db
            .revoke_registration(discord_id, crate::time::now())
            .await?
            .is_none()
        {
            return Ok(false);
        }
        if remove_role {
            self.release_role(discord_id).await?;
        } else if !self.cfg.member_role_id.is_empty() {
            self.db
                .queue_role_removal(discord_id, crate::time::now())
                .await?;
        }
        Ok(true)
    }

    pub async fn flag(&self, spec: &site_flags::Spec) -> Result<bool, DbError> {
        crate::settings::site_bool(&self.db, spec).await
    }

    /// 每小时领取 Key 总数的硬上限：把短时间的授权/领取突增抹平（2026-10-10 Discord 应用因「增长异常」被标记，
    /// 就是网页登录上线后一小时内 ~43 人集中授权触发的）。0 = 关闭。默认 12/小时，远高于自然速率、远低于会触发风控的突增。
    pub async fn issue_rate_blocked(&self, now: f64) -> Result<bool, DbError> {
        let cap = crate::settings::site_f64(&self.db, &site_flags::ISSUE_HOURLY_CAP).await?;
        if cap <= 0.0 {
            return Ok(false);
        }
        Ok(self.db.registrations_since(now - 3600.0).await? as f64 >= cap)
    }

    /// 站长发的邀请 Key（带「邀请」标签）还没绑定 Discord 账号。
    pub async fn is_unbound_invite(&self, key_id: i64) -> Result<bool, DbError> {
        self.db.is_unbound_invite(key_id).await
    }

    /// 绑定已有的邀请凭据。调用方必须先通过与普通领取相同的资格、容量和限流检查。
    pub async fn bind_invite(
        &self,
        user_id: &str,
        token: &str,
        user: &DiscordUser,
        cfg: &Registration,
    ) -> Result<Issued, RegErr> {
        let token = strip(token);
        let row = if token.starts_with("nai-") {
            self.db.key_id_enabled_by_token(token).await?
        } else {
            None
        };
        let Some((key_id, enabled)) = row else {
            return denied(
                "这不是一把待绑定的邀请 Key。请检查是否复制完整；普通领取请不填这一项。",
            );
        };
        if !self.db.is_unbound_invite(key_id).await? {
            return denied(
                "这不是一把待绑定的邀请 Key。请检查是否复制完整；普通领取请不填这一项。",
            );
        }
        if !enabled {
            return denied("这把邀请 Key 已被停用。");
        }
        let (username, display, avatar) = (
            clip(&user.username),
            clip(&user.global_name),
            clip(&user.avatar),
        );
        self.db
            .save_registration(&NewRegistration {
                discord_id: user_id,
                key_id,
                now: crate::time::now(),
                username: username.as_deref(),
                display_name: display.as_deref(),
                avatar: avatar.as_deref(),
                key_name: &member_label(user),
                expires_at: (cfg.expires_days > 0)
                    .then(|| crate::time::now() + cfg.expires_days as f64 * 86400.0),
                member_role: !self.cfg.member_role_id.is_empty(),
            })
            .await?;
        if !self.cfg.member_role_id.is_empty() {
            self.set_role(user_id, true).await;
        }
        Ok(Issued::Bound {
            key_id,
            message: "✅ 邀请 Key 已绑定到你的 Discord 账号，Key 不变，客户端里不用改。\
                      登录网站后可在右上角账户菜单复制 Key、查看额度；Key 泄露可用 /resetkey 重置。"
                .into(),
        })
    }

    /// /register 直接发 Key：斜杠命令的 interaction 已被 Discord 签名验明发起人身份，
    /// 不必再走 OAuth 授权（应用被标记审查期间 OAuth 被封；这条通路同时去掉了批量私信）。
    /// 这是唯一的发 Key 入口（旧的 OAuth 领取 begin/finish 已删除，不能再恢复）。
    ///
    /// 校验：限服务器 / 账号年龄 / 封禁 / 去重 / 封禁号关联（[`crate::ban_link`]）/ 每小时上限 / 名额 / 可选身份组（机器人 Token 核验）；
    /// 通过后交给 [`Self::provision`] 落库。不通过返回 [`RegErr::Denied`]。
    pub async fn issue_direct(
        &self,
        user_id: &str,
        guild_id: &str,
        user: &DiscordUser,
        invite_key: &str,
    ) -> Result<Issued, RegErr> {
        if guild_id != self.cfg.guild_id || !validation::is_decimal(user_id) {
            return denied("请在指定服务器使用 /register。");
        }
        if defaults::REGISTER_MIN_ACCOUNT_DAYS != 0 {
            // Discord 雪花 ID 里带创建时间；读不出创建时间一律拒绝
            let age_days =
                snowflake_created_at(user_id).map(|t| (crate::time::now() - t) / 86_400.0);
            if age_days.is_none_or(|d| d < defaults::REGISTER_MIN_ACCOUNT_DAYS as f64) {
                return denied(format!(
                    "Discord 账号创建满 {} 天后才能领 Key，请稍后再来。",
                    defaults::REGISTER_MIN_ACCOUNT_DAYS
                ));
            }
        }
        let _g = self.lock.lock().await;
        if let Some(reason) = self.ban_reason(user_id).await? {
            return denied(gate_core::idle::banned_notice(&reason));
        }
        self.release_if_expired(user_id, true).await?;
        if self.db.registration_key_id(user_id).await?.is_some() {
            return denied(
                "这个 Discord 账号已经领取过 Key 了。额度在网页登录后查看；Key 找不到的话，用 /resetkey 换一把新的。",
            );
        }
        // 和封禁号同浏览器 / 同（非苹果）设备登录过：不发 Key、不绑邀请（站长在待审里忽略那一对 = 放行）
        if crate::ban_link::blocked(&self.db, user_id).await? {
            return denied(crate::ban_link::BLOCK_NOTICE);
        }
        if self.issue_rate_blocked(crate::time::now()).await? {
            return denied(
                "本小时领取人数较多，为保护服务稳定已暂时限流，请过几分钟再用 /register 领取。",
            );
        }
        let cfg = self.check_capacity().await?;
        // 身份组门槛（后台「领 Key」可配，可跨服，可有多条、满足任意一条即可）：
        // 机器人不在那些服务器里，改查成员网页授权后记下的核验结果
        let rules = cfg.role_rules();
        if !rules.is_empty() {
            let mut passed = false;
            for rule in &rules {
                if self
                    .db
                    .role_verified_since(
                        user_id,
                        &rule.guild,
                        &rule.role,
                        crate::time::now() - 900.0,
                    )
                    .await?
                {
                    passed = true;
                    break;
                }
            }
            if !passed {
                return denied(format!(
                    "目前只开放给「{}」：请先到 {}login 用 Discord 登录（授权时会核验身份组），再用 /register 领取。",
                    cfg.role_note_text(),
                    self.cfg.site_url
                ));
            }
        }
        if !invite_key.is_empty() {
            return self.bind_invite(user_id, invite_key, user, &cfg).await;
        }
        let user = DiscordUser {
            id: user_id.to_string(),
            ..user.clone()
        };
        Ok(Issued::Provisioned(
            self.provision(user_id, &cfg, &user).await?,
        ))
    }

    /// 唯一发 Key 的地方：建 api_keys 行、写 discord_registrations、按成员名改 Key 名、
    /// 身份组标志 / 清掉旧的待摘除、挂身份组、生成欢迎消息。
    /// 调用方必须已持有 `lock` 并完成全部校验。
    pub async fn provision(
        &self,
        user_id: &str,
        cfg: &Registration,
        user: &DiscordUser,
    ) -> Result<Provisioned, RegErr> {
        let key = gate_core::policy::gen_key("nai", &rand::random::<[u8; 24]>());
        let created = crate::time::now();
        let row = self
            .db
            .create_key(
                &NewKey {
                    name: format!("Discord:{user_id}"),
                    token: key.clone(),
                    enabled: true,
                    expires_at: (cfg.expires_days > 0)
                        .then(|| created + (cfg.expires_days * 86400) as f64),
                    discord_id: Some(user_id.to_string()),
                },
                created,
            )
            .await?;
        let key_id = row.get("id").and_then(Value::as_i64).unwrap_or(0);
        let (username, display, avatar) = (
            clip(&user.username),
            clip(&user.global_name),
            clip(&user.avatar),
        );
        let stored = self
            .db
            .save_registration(&NewRegistration {
                discord_id: user_id,
                key_id,
                now: crate::time::now(),
                username: username.as_deref(),
                display_name: display.as_deref(),
                avatar: avatar.as_deref(),
                key_name: &member_label(user),
                expires_at: (cfg.expires_days > 0)
                    .then_some(created + cfg.expires_days as f64 * 86400.0),
                member_role: !self.cfg.member_role_id.is_empty(),
            })
            .await;
        if let Err(e) = stored {
            // Key 已建好但登记失败：不留没人认领的孤儿 Key
            let _ = self.db.delete_key(key_id, crate::time::now()).await;
            return Err(e.into());
        }
        if !self.cfg.member_role_id.is_empty() {
            self.set_role(user_id, true).await;
        }
        let policy = self.db.member_policy(user_id).await?.effective;
        let shared = self.db.shared_budgets().await?;
        let opened = features::FEATURES
            .iter()
            .filter(|(name, _)| policy.allows(name))
            .map(|(_, label)| *label)
            .collect::<Vec<_>>()
            .join("、");
        let v5 = policy.allows("v5")
            && !crate::ops::v5_paused(&self.db).await?
            && policy.daily_v5 > 0
            && shared.daily_v5 > 0;
        let images_available = policy.daily_images > 0 && policy.image;
        let daily_images = i64::from(policy.daily_images);
        let daily_v5 = i64::from(policy.daily_v5);
        let message = welcome_message(&Welcome {
            key: &key,
            site: &self.cfg.site_url,
            daily_images,
            images_available,
            v5,
            daily_v5,
            reset_hour: gate_core::clock::DAY_RESET_HOUR,
            idle_days: self.env.key_inactivity_delete_days,
            opened: &opened,
        });
        Ok(Provisioned { key, message })
    }
}
