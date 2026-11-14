//! One permission-checked registry for native commands and model tools.
//! 全部只读，唯一例外是 `report_client`：只写本人的「自报客户端」后台标签，不改额度或权限。
use super::*;
use serde::{Deserialize, Serialize};
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolRequest {
    pub actor: String,
    pub channel: String,
    pub name: String,
    #[serde(default)]
    pub arguments: Value,
}
#[derive(Clone, Copy)]
pub struct Capability {
    pub name: &'static str,
    pub command: &'static str,
    pub description: &'static str,
    pub source: &'static str,
    pub timeout: u64,
}
pub const CAPABILITIES: &[Capability] = &[
    Capability {
        name: "share_status",
        command: "分享状态",
        description: "查询本人分享履约和限制，不重置额度",
        source: "owl",
        timeout: 8,
    },
    Capability {
        name: "site_status",
        command: "站点状态",
        description: "查询实时站点排队和出图统计",
        source: "owl",
        timeout: 8,
    },
    Capability {
        name: "member_status",
        command: "我的额度",
        description: "查询本人额度、钱包与签到",
        source: "owl/newapi",
        timeout: 8,
    },
    Capability {
        name: "upstream_status",
        command: "上游状态",
        description: "查询 NewAPI 模型近期请求健康，开放不代表可用",
        source: "newapi",
        timeout: 8,
    },
    Capability {
        name: "activity",
        command: "活跃度",
        description: "查询公开活动事实，不能判定作者应否分享",
        source: "discord",
        timeout: 8,
    },
    Capability {
        name: "creator_audit",
        command: "作者足迹",
        description: "查询作者相关的可见原生转发、回复和提及；下载者不可查询",
        source: "discord",
        timeout: 8,
    },
    Capability {
        name: "report_client",
        command: "报客户端",
        description: "登记本人使用的生图客户端名称（被要求说明客户端时用）。额度被风控临时清零时，登记后自动恢复默认额度",
        source: "owl",
        timeout: 8,
    },
    Capability {
        name: "bot_status",
        command: "奶妹状态",
        description: "查询当前生产版本、对话模型、Discord 连接和分享及聊天队列健康",
        source: "bot",
        timeout: 8,
    },
];
pub fn tools() -> Value {
    json!(CAPABILITIES.iter().map(|c|json!({"type":"function","function":{"name":c.name,"description":c.description,"parameters":if c.name=="report_client"{json!({"type":"object","properties":{"client":{"type":"string","description":"成员亲口说出的客户端名称，原样填写，不要猜"}},"required":["client"],"additionalProperties":false})}else{json!({"type":"object","properties":if matches!(c.name,"activity"|"creator_audit"){json!({"target":{"type":"string","description":"待查成员 ID；作者足迹在帖子中默认查楼主"},"before":{"type":"string","description":"仅明确指定截止日期时传 YYYY-MM-DD；默认全部可见历史"}})}else{json!({})},"additionalProperties":false})}}})).collect::<Vec<_>>())
}
impl Naimei {
    pub async fn capability(self: &Arc<Self>, r: ToolRequest) -> Value {
        let Ok(_slot) = self.query_slots.try_acquire() else {
            return json!({"error":"只读查询繁忙，请稍后重试；分享处理使用独立预算"});
        };
        if std::env::var("NAIMEI_QUERIES").as_deref() == Ok("0") {
            return json!({"error":"查询模块已关闭"});
        }
        let Some(c) = CAPABILITIES.iter().find(|c| c.name == r.name) else {
            return json!({"error":"unknown capability"});
        };
        if !valid_id(&r.actor) || !valid_id(&r.channel) {
            return json!({"error":"invalid actor or channel"});
        }
        if !r.arguments.is_null()
            && (!r.arguments.is_object()
                || r.arguments.as_object().is_some_and(|o| {
                    o.keys().any(|k| {
                        !((matches!(c.name, "activity" | "creator_audit")
                            && matches!(k.as_str(), "target" | "before"))
                            || (c.name == "share_status" && k == "message_id")
                            || (c.name == "report_client" && k == "client"))
                    })
                }))
        {
            return json!({"error":"invalid arguments"});
        }
        let result =
            tokio::time::timeout(Duration::from_secs(c.timeout), self.capability_inner(&r)).await;
        match result {
            Ok(v) => json!({"source":c.source,"sampled_at":self.now(),"data":v}),
            Err(_) => {
                json!({"source":c.source,"sampled_at":self.now(),"error":"查询超时，状态未知"})
            }
        }
    }
    async fn capability_inner(self: &Arc<Self>, r: &ToolRequest) -> Value {
        match r.name.as_str() {
            "site_status" => self
                .bridge
                .site_stats()
                .await
                .unwrap_or(json!({"error":"站点数据暂不可用"})),
            "share_status" => {
                let Some(v) = self
                    .bridge
                    .member_context_message(
                        &r.actor,
                        self.cfg.guild_id,
                        r.arguments["message_id"].as_str(),
                    )
                    .await
                else {
                    return json!({"error":"本人状态暂不可用"});
                };
                let rule = &v["member"]["share_requirement"];
                json!({"share_requirement":rule,"other_limits":v["member"],"note":if rule["required_now"]==true{"昨日和今日尚未履约，请到指定频道分享"}else{"当前无需分享或分享限制已解除；额度、封禁等其他限制仍独立有效。分享不会重置额度。"}})
            }
            "member_status" => {
                let (owl, newapi) = tokio::join!(
                    self.bridge.member_context(&r.actor, self.cfg.guild_id),
                    self.bridge.newapi_context(&self.cfg.site, &r.actor)
                );
                json!({"owl":owl,"newapi":newapi})
            }
            "upstream_status" => self
                .bridge
                .newapi_health(&self.cfg.site, self.is_admin(&r.actor).await)
                .await
                .unwrap_or(json!({"error":"上游状态暂不可用，不能推断模型正常"})),
            "bot_status" => {
                let mut v = json!({"build":{"version":env!("CARGO_PKG_VERSION"),"commit":std::env::var("OWL_BUILD_SHA").ok().filter(|s|s.len()==40&&s.bytes().all(|b|b.is_ascii_hexdigit())),"chat_model":self.chat.as_ref().map(Chat::model)},"commands":crate::commands::definitions().iter().map(|d|json!({"name":d["name"],"type":d["type"],"description":d["description"]})).collect::<Vec<_>>(),"model_tools":CAPABILITIES.iter().map(|c|c.name).collect::<Vec<_>>(),"discord_connected":self.connected(),"share_detection":self.share_detection_report(),"chat":self.chat_status(),"reactions":self.reaction_status()});
                if self.is_admin(&r.actor).await {
                    v["resources"] = resources();
                }
                v
            }
            "activity" | "creator_audit" => self.activity_query(r).await,
            "report_client" => match r.arguments["client"].as_str().map(str::trim) {
                Some(c) if !c.is_empty() => {
                    self.bridge
                        .report_client(&r.actor, self.cfg.guild_id, c)
                        .await
                }
                _ => json!({"error":"请写上客户端名称"}),
            },
            _ => json!({"error":"unknown capability"}),
        }
    }
    pub(super) async fn is_admin(&self, uid: &str) -> bool {
        if uid
            .parse::<u64>()
            .is_ok_and(|id| self.cfg.owner_ids.contains(&id))
        {
            return true;
        }
        let Ok((_, member)) = self
            .api
            .read(format!("guilds/{}/members/{uid}", self.cfg.guild_id))
            .await
        else {
            return false;
        };
        let Ok((_, roles)) = self
            .api
            .read(format!("guilds/{}/roles", self.cfg.guild_id))
            .await
        else {
            return false;
        };
        let ids = member["roles"].as_array().cloned().unwrap_or_default();
        roles.as_array().is_some_and(|roles| {
            roles
                .iter()
                .any(|r| ids.contains(&r["id"]) && bits(&r["permissions"]) & (8 | 32) != 0)
        })
    }
}
fn valid_id(s: &str) -> bool {
    (17..=20).contains(&s.len())
        && s.bytes().all(|b| b.is_ascii_digit())
        && s.parse::<u64>().is_ok()
}
pub(super) fn bits(v: &Value) -> u64 {
    v.as_str()
        .and_then(|s| s.parse().ok())
        .or_else(|| v.as_u64())
        .unwrap_or(0)
}
fn resources() -> Value {
    let mem = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let load = std::fs::read_to_string("/proc/loadavg").unwrap_or_default();
    json!({"load":load.split_whitespace().take(3).collect::<Vec<_>>(),"memory":mem.lines().filter(|l|l.starts_with("MemTotal:")||l.starts_with("MemAvailable:")).collect::<Vec<_>>(),"note":"主机瞬时采样，不是 API 账单或月费"})
}

pub fn render(name: &str, result: &Value) -> String {
    let data = &result["data"];
    // 顶层 error 只来自超时分支：查询类如实说超时；report_client 是写操作，
    // 超时可能已写库成功，不能说成「登记失败」，也劝成员别立刻重试。
    if let Some(err) = result["error"].as_str() {
        return if name == "report_client" {
            "登记超时，结果未知；请等一分钟后再试，若提示已登记就不要重复提交。".into()
        } else {
            format!("查询超时，状态未知：{err}。")
        };
    }
    if data["status"] == "pending" {
        return "已接收，正在后台核对全部可见历史。完成后会返回结果；查不到或未覆盖的部分不会当作没有发生。".into();
    }
    if name == "creator_audit" && data.get("counts").is_some() {
        let mut out = format!(
            "**作者相关行为核对**\n对象：`{}`\n范围：本服务器中双方均可读的服务器频道与公开帖子（含身份组可见频道）；默认全部可见历史。\n结果：{}\n\n**已核实行为**\n",
            data["target_name"]
                .as_str()
                .or_else(|| data["target"].as_str())
                .unwrap_or("未知"),
            if data["complete"] == true {
                "本次搜索已完成（不包含已删除或不可见内容）"
            } else {
                "部分结果，不能证明没有其他行为"
            }
        );
        if let Some(rows) = data["counts"].as_object() {
            for (kind, v) in rows {
                out.push_str(&format!(
                    "- {kind}：{} 次，涉及 {} 位成员。\n",
                    v["messages"], v["members"]
                ));
            }
        }
        if let Some(at) = data["until"].as_f64() {
            out.push_str(&format!(
                "\n核对截止：{}（北京时间）。\n",
                gate_core::clock::Clock::service().format(at, "%Y-%m-%d %H:%M:%S")
            ));
        }
        out.push_str("\n**证据（最多列出 10 条）**\n");
        if let Some(rows) = data["evidence"].as_array() {
            if rows.is_empty() {
                out.push_str("本次尚未找到可核实证据；这不是没有发生的证明。\n");
            }
            for row in rows.iter().take(10) {
                let kinds = row["kinds"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str())
                            .collect::<Vec<_>>()
                            .join("、")
                    })
                    .unwrap_or_default();
                let who = row["member_name"]
                    .as_str()
                    .unwrap_or("未知")
                    .replace('`', "'")
                    .replace(['\n', '\r'], " ");
                out.push_str(&format!(
                        "- `{who}`（`{}`）：{kinds}，{}。[消息证据]({})\n",
                        row["member_id"].as_str().unwrap_or("未知"),
                        row["at_unix"]
                            .as_f64()
                            .map(|at| gate_core::clock::Clock::service()
                                .format(at, "%Y-%m-%d %H:%M:%S"))
                            .as_deref()
                            .unwrap_or("时间未知"),
                        row["message_url"].as_str().unwrap_or("")
                    ));
            }
        }
        out.push_str("\n**无法查询：谁下载、保存或截图了附件；私信、其他服务器及站外二传。** Discord 不提供这些记录，不能报成 0。回复、提及和转发可能重叠，人数不能相加；转发事实不等于违规结论，由作者自行判断。\n");
        if data["complete"] != true {
            let issues = data["issues"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str())
                        .collect::<Vec<_>>()
                        .join("；")
                })
                .unwrap_or_default();
            if !issues.is_empty() {
                out.push_str(&format!("\n未完整核实：{issues}。"));
            }
            out.push_str(&format!(
                "\n有 {} 条原消息因删除、权限或来源不可读而未能核实；这些不能当作零记录。",
                data["unverified_origins"].as_i64().unwrap_or(0)
            ));
        }
        return out;
    }
    // creator_audit 但没有 counts（历史核对超时 / 后端报错）：给成员可读的说明，不吐原始 JSON。
    if name == "creator_audit" {
        return format!(
            "历史核对尚未完成：{}。请稍后重新查询；未完成的核对不能当作没有相关行为。",
            data["error"].as_str().unwrap_or("后台仍在处理")
        );
    }
    if name == "report_client" {
        return match data["client"].as_str() {
            Some(c) if data["recorded"] == true => format!(
                "已登记你的客户端：**{}**（连汇报时间和近期实际请求记录一起写进后台标签，站长核对后处理；登记本身不会自动解除限制）。",
                c.replace('`', "'")
            ),
            _ => format!(
                "登记失败：{}",
                data["error"].as_str().unwrap_or("暂不可用，请稍后再试")
            ),
        };
    }
    serde_json::to_string_pretty(result).unwrap_or_else(|_| "查询结果暂不可用".into())
}
impl Naimei {
    pub(super) async fn activity_result(self: &Arc<Self>, request: ToolRequest) -> Value {
        let until = tokio::time::Instant::now() + Duration::from_secs(100);
        loop {
            tokio::time::sleep(Duration::from_secs(2)).await;
            let v = self.capability(request.clone()).await;
            if v["data"]["status"] != "pending" {
                return v;
            }
            if tokio::time::Instant::now() >= until {
                return json!({"data":{"complete":false,"error":"历史查询尚未完成，请稍后再查；不能推断没有相关行为"}});
            }
        }
    }
}

pub fn wants_creator_audit(text: &str) -> bool {
    ["查", "查询", "核对", "核查"]
        .iter()
        .any(|s| text.contains(s))
        && ["转发", "下载", "二传", "谁保存", "谁截图", "作者足迹"]
            .iter()
            .any(|s| text.contains(s))
}
