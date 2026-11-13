//! Historical facts: permission-filtered, paginated, partial results are explicit.
use super::capabilities::{ToolRequest, bits};
use super::*;
#[derive(Default)]
pub(super) struct Activity {
    pub cache: HashMap<String, (f64, Value)>,
}
impl Naimei {
    pub(super) async fn activity_query(self: &Arc<Self>, r: &ToolRequest) -> Value {
        let ch = match r.channel.parse() {
            Ok(id) => id,
            Err(_) => return json!({"error":"频道无效"}),
        };
        let info = self.api.channel_info(ch).await.unwrap_or(Value::Null);
        if info["guild_id"].as_str() != Some(&self.cfg.guild_id.to_string()) {
            return json!({"error":"无法核实服务器"});
        }
        let audit = r.name == "creator_audit";
        let target = r.arguments["target"]
            .as_str()
            .or_else(|| {
                if audit {
                    info["owner_id"].as_str()
                } else {
                    Some(r.actor.as_str())
                }
            })
            .unwrap_or(&r.actor)
            .to_owned();
        if target.len() < 17 || target.len() > 20 || target.parse::<u64>().is_err() {
            return json!({"error":"成员 ID 无效"});
        }
        let cutoff = if let Some(day) = r.arguments["before"].as_str() {
            match gate_core::clock::Clock::service().local_midnight(day) {
                Some(at) if at >= 1_420_070_400.0 && at <= self.now() => at,
                _ => return json!({"error":"截止日期应为有效的 YYYY-MM-DD，且不能晚于当前日期"}),
            }
        } else {
            self.now()
        };
        if target != r.actor
            && !(info["owner_id"] == r.actor && info["type"] != 12)
            && !self.is_admin(&r.actor).await
        {
            return json!({"error":"仅可查本人；帖子作者可在本人帖子内查指定成员，管理员可授权查询"});
        }
        let key = format!(
            "{}:{}:{target}:{}:{}",
            r.name,
            r.actor,
            r.channel,
            r.arguments["before"].as_str().unwrap_or("all")
        );
        {
            let mut a = self
                .activity
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            a.cache.retain(|key, (at, v)| {
                self.now() - *at
                    < if v["status"] == "pending" {
                        120.0
                    } else if key.starts_with("creator_audit:") {
                        30.0
                    } else {
                        300.0
                    }
            });
            if let Some((_, v)) = a.cache.get(&key) {
                return v.clone();
            }
            if a.cache.len() >= 32 {
                return json!({"error":"历史查询繁忙，请稍后重试"});
            }
            a.cache.insert(key.clone(),(self.now(),json!({"status":"pending","note":"正在后台核对，稍后重新使用相同命令查看；不影响分享检测"})));
        }
        let app = self.clone();
        let actor = r.actor.clone();
        let jobkey = key.clone();
        let source = if matches!(info["type"].as_u64(), Some(10 | 11)) {
            r.channel.clone()
        } else {
            String::new()
        };
        tokio::spawn(async move {
            let result = tokio::time::timeout(Duration::from_secs(90), async {
                if audit {
                    app.scan_creator(&actor, &target, &source, cutoff, &jobkey)
                        .await
                } else {
                    app.scan_activity(&actor, &target, cutoff).await
                }
            })
            .await
            .unwrap_or_else(|_| {
                let mut progress = app
                    .activity
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .cache
                    .get(&jobkey)
                    .map(|(_, v)| v.clone())
                    .unwrap_or(json!({}));
                progress["status"] = json!("finished");
                progress["complete"] = json!(false);
                progress["error"] =
                    json!("历史查询超时，以下仅为已核实的部分结果，不能推断没有其他行为");
                progress
            });
            app.activity
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .cache
                .insert(jobkey, (app.now(), result));
        });
        json!({"status":"pending","note":"正在后台查询，请稍后用相同命令查看。默认全部可见历史；结果完成后会标注覆盖范围。"})
    }
    async fn scan_activity(&self, actor: &str, target: &str, cutoff: f64) -> Value {
        let _slot = self
            .activity_slots
            .acquire()
            .await
            .expect("activity budget");
        let max = ((cutoff * 1000.0) as u64 - 1_420_070_400_000) << 22;
        let Ok((_, roles)) = self
            .api
            .read(format!("guilds/{}/roles", self.cfg.guild_id))
            .await
        else {
            return json!({"complete":false,"error":"无法核实公开频道权限"});
        };
        let Ok((_, member)) = self
            .api
            .read(format!("guilds/{}/members/{actor}", self.cfg.guild_id))
            .await
        else {
            return json!({"complete":false,"error":"无法核实查询者权限"});
        };
        let Some(bot) = self.lock().bot_id.clone() else {
            return json!({"complete":false,"error":"Bot 未就绪"});
        };
        let Ok((_, bot_member)) = self
            .api
            .read(format!("guilds/{}/members/{bot}", self.cfg.guild_id))
            .await
        else {
            return json!({"complete":false,"error":"无法核实 Bot 权限"});
        };
        let mut count = 0usize;
        let mut images = 0usize;
        let mut days = HashSet::new();
        let mut links = Vec::new();
        let mut first: Option<String> = None;
        let mut last: Option<String> = None;
        let mut seen = HashSet::new();
        let mut channels: HashMap<String, Value> = HashMap::new();
        let mut complete = false;
        let mut issue = None;
        let mut offset = 0;
        while offset <= 9975 {
            let path = format!(
                "guilds/{}/messages/search?author_id={target}&max_id={max}&limit=25&offset={offset}&include_nsfw=true&sort_by=timestamp&sort_order=asc",
                self.cfg.guild_id
            );
            let response = tokio::time::timeout(Duration::from_secs(8), self.api.read(path)).await;
            let Ok(Ok((status, page))) = response else {
                issue = Some("搜索暂不可用");
                break;
            };
            if status == 202 {
                let delay = page["retry_after"].as_f64().unwrap_or(2.0).clamp(1.0, 10.0);
                tokio::time::sleep(Duration::from_secs_f64(delay)).await;
                continue;
            }
            if page["doing_deep_historical_index"] == true {
                issue = Some("Discord 正在建立历史索引");
                break;
            }
            let Some(groups) = page["messages"].as_array() else {
                issue = Some("搜索结果无效");
                break;
            };
            let total = page["total_results"].as_u64().unwrap_or(u64::MAX);
            let rows: Vec<Value> = groups
                .iter()
                .filter_map(|g: &Value| g.as_array())
                .flat_map(|a| a.iter().cloned())
                .collect();
            for raw in &rows {
                let Some(mid) = raw["id"].as_str() else {
                    continue;
                };
                if !seen.insert(mid.to_owned()) {
                    continue;
                }
                let Some(cid) = raw["channel_id"].as_str() else {
                    continue;
                };
                if !channels.contains_key(cid) {
                    let info = self
                        .api
                        .channel_info(cid.parse().unwrap_or(0))
                        .await
                        .unwrap_or(Value::Null);
                    channels.insert(cid.into(), info);
                }
                let mut info = channels[cid].clone();
                if info.is_null() {
                    issue = Some("部分频道权限无法核实");
                    continue;
                }
                if matches!(info["type"].as_u64(), Some(10..=12)) {
                    if info["type"] == 12 {
                        continue;
                    }
                    let Some(parent) = info["parent_id"].as_str() else {
                        issue = Some("帖子父频道未知");
                        continue;
                    };
                    if !channels.contains_key(parent) {
                        let p = self
                            .api
                            .channel_info(parent.parse().unwrap_or(0))
                            .await
                            .unwrap_or(Value::Null);
                        channels.insert(parent.into(), p);
                    }
                    info = channels[parent].clone();
                }
                if !visible(
                    &roles,
                    &member,
                    &info,
                    actor,
                    &self.cfg.guild_id.to_string(),
                ) || !visible(
                    &roles,
                    &bot_member,
                    &info,
                    &bot,
                    &self.cfg.guild_id.to_string(),
                ) {
                    continue;
                }
                let Some(at) = share::snowflake_unix(mid) else {
                    issue = Some("消息时间无效");
                    continue;
                };
                if at >= cutoff {
                    continue;
                }
                count += 1;
                let day = gate_core::clock::Clock::service().day(at + 7.0 * 3600.0);
                days.insert(day);
                let stamp = raw["timestamp"].as_str().unwrap_or("").to_owned();
                if first.as_ref().is_none_or(|s| s > &stamp) {
                    first = Some(stamp.clone());
                }
                if last.as_ref().is_none_or(|s| s < &stamp) {
                    last = Some(stamp);
                }
                if serde_json::from_value::<MessageInfo>(raw.clone())
                    .ok()
                    .is_some_and(|m| share::has_image(&m))
                {
                    images += 1;
                }
                if links.len() < 5 {
                    links.push(format!(
                        "https://discord.com/channels/{}/{cid}/{mid}",
                        self.cfg.guild_id
                    ));
                }
            }
            offset += 25;
            if offset as u64 >= total {
                complete = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        json!({"status":"finished","target":target,"cutoff":cutoff,"timezone":"Asia/Shanghai","scope":"Bot 与查询者均可见的可读频道及公开帖子（包含身份组可见频道，排除私有帖子）","messages":count,"active_days":days.len(),"image_shares":images,"first_at":first,"last_at":last,"links":links,"complete":complete&&issue.is_none(),"issue":issue,"sampled_at":self.now(),"note":"只统计可读取且仍存在的内容，不评定是否合格；不完整结果是下限，不能视为零活跃证明"})
    }
}
fn base(roles: &Value, ids: &[Value], guild: &str) -> u64 {
    roles
        .as_array()
        .map(|rr| {
            rr.iter()
                .filter(|r| r["id"] == guild || ids.contains(&r["id"]))
                .fold(0, |acc, r| acc | bits(&r["permissions"]))
        })
        .unwrap_or(0)
}
fn overwrite(mut p: u64, ch: &Value, ids: &[Value], uid: &str, guild: &str) -> u64 {
    let rows = ch["permission_overwrites"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    for r in rows.iter().filter(|r| r["id"] == guild) {
        p = (p & !bits(&r["deny"])) | bits(&r["allow"]);
    }
    let (deny, allow) = rows
        .iter()
        .filter(|r| r["type"] == 0 && ids.contains(&r["id"]))
        .fold((0, 0), |(d, a), r| {
            (d | bits(&r["deny"]), a | bits(&r["allow"]))
        });
    p = (p & !deny) | allow;
    for r in rows.iter().filter(|r| r["type"] == 1 && r["id"] == uid) {
        p = (p & !bits(&r["deny"])) | bits(&r["allow"]);
    }
    p
}
pub(super) fn visible(roles: &Value, m: &Value, ch: &Value, uid: &str, guild: &str) -> bool {
    let ids = m["roles"].as_array().cloned().unwrap_or_default();
    let p = base(roles, &ids, guild);
    p & 8 != 0
        || overwrite(p, ch, &ids, uid, guild) & ((1 << 10) | (1 << 16)) == ((1 << 10) | (1 << 16))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn private_channels_and_role_denials_are_excluded() {
        let roles = json!([{"id":"1","permissions":((1u64<<10)|(1<<16)).to_string()}]);
        let ch = json!({"permission_overwrites":[{"id":"1","type":0,"deny":(1u64<<10).to_string(),"allow":"0"}]});
        assert!(!visible(&roles, &json!({"roles":[]}), &ch, "2", "1"));
    }
}

#[cfg(test)]
mod member_visible_tests {
    use super::*;
    #[test]
    fn member_only_forum_is_included_for_qualified_reader() {
        let roles = json!([{"id":"1","permissions":"0"},{"id":"3","permissions":((1u64<<10)|(1<<16)).to_string()}]);
        let forum = json!({"permission_overwrites":[{"id":"1","type":0,"deny":(1u64<<10).to_string(),"allow":"0"},{"id":"3","type":0,"deny":"0","allow":(1u64<<10).to_string()}]});
        assert!(visible(&roles, &json!({"roles":["3"]}), &forum, "2", "1"));
        assert!(!visible(&roles, &json!({"roles":[]}), &forum, "2", "1"));
    }
}
