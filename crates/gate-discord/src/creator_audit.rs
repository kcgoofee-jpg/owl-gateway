//! Evidence of visible interactions, never a download tracker or an infringement verdict.
use super::activity::{public_visible, visible};
use super::*;
struct Scope {
    roles: Value,
    member: Value,
    bot_member: Value,
    actor: String,
    bot: String,
    channels: HashMap<String, Value>,
}
impl Naimei {
    async fn audit_visible(&self, cid: &str, s: &mut Scope) -> Option<bool> {
        if !s.channels.contains_key(cid) {
            let ch = self.api.channel_info(cid.parse().ok()?).await.ok()?;
            s.channels.insert(cid.to_owned(), ch);
        }
        let mut ch = s.channels.get(cid)?.clone();
        if ch["guild_id"].as_str() != Some(&self.cfg.guild_id.to_string()) {
            return Some(false);
        }
        if matches!(ch["type"].as_u64(), Some(10..=12)) {
            if ch["type"] == 12 {
                return Some(false);
            }
            let parent = ch["parent_id"].as_str()?;
            if !s.channels.contains_key(parent) {
                let p = self.api.channel_info(parent.parse().ok()?).await.ok()?;
                s.channels.insert(parent.into(), p);
            }
            ch = s.channels.get(parent)?.clone();
        }
        let gid = self.cfg.guild_id.to_string();
        Some(
            public_visible(&s.roles, &ch, &gid)
                && visible(&s.roles, &s.member, &ch, &s.actor, &gid)
                && visible(&s.roles, &s.bot_member, &ch, &s.bot, &gid),
        )
    }
    pub(super) async fn scan_creator(
        &self,
        actor: &str,
        target: &str,
        source: &str,
        cutoff: f64,
        jobkey: &str,
    ) -> Value {
        let _slot = self
            .activity_slots
            .acquire()
            .await
            .expect("activity budget");
        let Some(bot) = self.lock().bot_id.clone() else {
            return json!({"complete":false,"error":"Bot 未就绪"});
        };
        let (roles, member, bot_member) = tokio::join!(
            self.api.read(format!("guilds/{}/roles", self.cfg.guild_id)),
            self.api
                .read(format!("guilds/{}/members/{actor}", self.cfg.guild_id)),
            self.api
                .read(format!("guilds/{}/members/{bot}", self.cfg.guild_id))
        );
        let (Ok((_, roles)), Ok((_, member)), Ok((_, bot_member))) = (roles, member, bot_member)
        else {
            return json!({"complete":false,"error":"无法核实查询权限"});
        };
        let mut scope = Scope {
            roles,
            member,
            bot_member,
            actor: actor.into(),
            bot,
            channels: HashMap::new(),
        };
        let max = ((cutoff * 1000.0) as u64 - 1_420_070_400_000) << 22;
        let mut queries = vec![
            ("原生转发", String::from("has=snapshot")),
            ("回复作者", format!("replied_to_user_id={target}")),
            ("提及作者", format!("mentions={target}")),
        ];
        if !source.is_empty() {
            queries.push((
                "该帖链接",
                format!("content={source}&link_hostname=discord.com"),
            ));
        }
        let mut counts = serde_json::Map::new();
        let mut evidence = std::collections::BTreeMap::<String, Value>::new();
        let mut origins = HashMap::<String, Value>::new();
        let mut issues = Vec::new();
        let mut scanned = 0;
        let mut unverified = 0;
        for (kind, query) in queries {
            let mut offset = 0;
            let mut complete = false;
            let mut ids = HashSet::new();
            let mut people = HashSet::new();
            while offset <= 9975 {
                let url = format!(
                    "guilds/{}/messages/search?{query}&author_type=user&max_id={max}&limit=25&offset={offset}&include_nsfw=true&sort_by=timestamp&sort_order=desc",
                    self.cfg.guild_id
                );
                let page = tokio::time::timeout(Duration::from_secs(8), self.api.read(url)).await;
                let Ok(Ok((status, page))) = page else {
                    issues.push(format!("{kind}搜索暂不可用"));
                    break;
                };
                if status == 202 {
                    tokio::time::sleep(Duration::from_secs_f64(
                        page["retry_after"].as_f64().unwrap_or(2.0).clamp(1.0, 10.0),
                    ))
                    .await;
                    continue;
                }
                if page["doing_deep_historical_index"] == true {
                    issues.push(format!("{kind}历史索引尚未就绪"));
                    break;
                }
                let Some(groups) = page["messages"].as_array() else {
                    issues.push(format!("{kind}搜索结果无效"));
                    break;
                };
                let rows: Vec<Value> = groups
                    .iter()
                    .filter_map(|g: &Value| g.as_array())
                    .flat_map(|a| a.iter().cloned())
                    .collect();
                for m in rows {
                    scanned += 1;
                    let (Some(id), Some(cid), Some(who)) = (
                        m["id"].as_str(),
                        m["channel_id"].as_str(),
                        m["author"]["id"].as_str(),
                    ) else {
                        continue;
                    };
                    if m["author"]["bot"] == true || !m["webhook_id"].is_null() {
                        continue;
                    }
                    match self.audit_visible(cid, &mut scope).await {
                        Some(true) => {}
                        Some(false) => continue,
                        None => {
                            issues.push("部分频道权限未能核实".into());
                            continue;
                        }
                    }
                    let mut source_link = None;
                    let relevant = if kind == "提及作者" {
                        m["mentions"]
                            .as_array()
                            .is_some_and(|a| a.iter().any(|u| u["id"] == target))
                    } else {
                        let reference = if kind == "该帖链接" {
                            link_reference(&m, self.cfg.guild_id, source)
                        } else {
                            let expected = if kind == "原生转发" { 1 } else { 0 };
                            let r = &m["message_reference"];
                            if r["type"].as_u64().unwrap_or(0) == expected {
                                Some(r.clone())
                            } else {
                                None
                            }
                        };
                        if let Some(r) = reference {
                            if let (Some(oc), Some(om)) =
                                (r["channel_id"].as_str(), r["message_id"].as_str())
                            {
                                if self.audit_visible(oc, &mut scope).await != Some(true) {
                                    unverified += 1;
                                    false
                                } else {
                                    let key = format!("{oc}/{om}");
                                    if !origins.contains_key(&key) {
                                        let value = match (oc.parse(), om.parse()) {
                                            (Ok(c), Ok(m)) => self
                                                .api
                                                .message_by_id(c, m)
                                                .await
                                                .unwrap_or(Value::Null),
                                            _ => Value::Null,
                                        };
                                        origins.insert(key.clone(), value);
                                    }
                                    let original = &origins[&key];
                                    if original.is_null() {
                                        unverified += 1;
                                        false
                                    } else {
                                        source_link = Some(format!(
                                            "https://discord.com/channels/{}/{oc}/{om}",
                                            self.cfg.guild_id
                                        ));
                                        original["author"]["id"] == target
                                    }
                                }
                            } else {
                                unverified += 1;
                                false
                            }
                        } else {
                            false
                        }
                    };
                    if !relevant || !ids.insert(id.to_owned()) {
                        continue;
                    }
                    people.insert(who.to_owned());
                    if evidence.len() < 50 || evidence.contains_key(id) {
                        let entry=evidence.entry(id.into()).or_insert_with(||json!({"member_id":who,"member_name":m["author"]["global_name"].as_str().or_else(||m["author"]["username"].as_str()).unwrap_or("未知"),"at":m["timestamp"],"message_url":format!("https://discord.com/channels/{}/{cid}/{id}",self.cfg.guild_id),"source_url":source_link,"kinds":[]}));
                        entry["kinds"]
                            .as_array_mut()
                            .expect("kinds")
                            .push(json!(kind));
                    }
                }
                counts.insert(
                    kind.into(),
                    json!({"messages":ids.len(),"members":people.len(),"complete":false}),
                );
                let partial = json!({"status":"pending","target":target,"until":cutoff,"sampled_at":self.now(),"counts":counts,"evidence":evidence.values().cloned().collect::<Vec<_>>(),"searched_rows":scanned,"unverified_origins":unverified,"complete":false,"issues":["历史查询仍在进行"],"unavailable":{"downloaders":null,"savers":null,"screenshots":null,"private_or_external_forwards":null}});
                self.activity
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .cache
                    .insert(jobkey.to_owned(), (self.now(), partial));
                offset += 25;
                if offset as u64 >= page["total_results"].as_u64().unwrap_or(u64::MAX) {
                    complete = true;
                    break;
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            if !complete {
                issues.push(format!("{kind}分页未完整结束"));
            }
            counts.insert(
                kind.into(),
                json!({"messages":ids.len(),"members":people.len(),"complete":complete}),
            );
        }
        issues.sort();
        issues.dedup();
        json!({"status":"finished","target":target,"scope":"本服务器中 Bot 与查询者均可读的公开频道和公开帖子；默认全部可见历史","until":cutoff,"sampled_at":self.now(),"counts":counts,"evidence":evidence.into_values().collect::<Vec<_>>(),"evidence_limit":50,"searched_rows":scanned,"unverified_origins":unverified,"complete":issues.is_empty()&&unverified==0,"issues":issues,"unavailable":{"downloaders":null,"savers":null,"screenshots":null,"private_or_external_forwards":null},"notes":["Discord 不提供谁下载、保存或截图了附件的数据；null 表示无法查询，不是零","原生转发须核实原消息发送人；快照本身没有原作者字段","各类行为可能重叠，不能相加作为总人数；可见转发不等于侵权","删帖、无权限、私信、其他服务器和站外行为未覆盖；无法靠现有接口证明没有二传"]})
    }
}
fn link_reference(m: &Value, guild: u64, source: &str) -> Option<Value> {
    let re =
        regex::Regex::new(r"https?://(?:www\.)?discord(?:app)?\.com/channels/(\d+)/(\d+)/(\d+)")
            .expect("regex");
    for c in re.captures_iter(m["content"].as_str().unwrap_or("")) {
        if c[1] == guild.to_string() && c[2] == *source {
            return Some(json!({"channel_id":&c[2],"message_id":&c[3]}));
        }
    }
    None
}
