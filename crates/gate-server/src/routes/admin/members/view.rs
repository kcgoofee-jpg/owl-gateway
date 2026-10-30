//! 成员页的视图模型：把 Key、今日 / 本周用量、规则、领取记录合成前端渲染用的 JSON。
//!
//! 「需要站长处理」「已查看 24 小时内豁免」这些口径和 `web/src/pages/admin/sections/members/logic.ts`
//! 一一对应，两边要一起改（各函数注释里写着对应的前端函数）。
//!
//! 路由只做参数校验和响应，聚合逻辑在这里。

use super::{AUTO_TAG_PATTERNS, discord_profile, jflag, jget};
use crate::error::ApiError;
use crate::routes::admin::req::http;
use crate::state::AppState;
use gate_core::{primitives, validation, values};
use gate_db::repo::INVITE_TAG;
use gate_db::repo::admin_members::{RegistrationRow, TagRow};
use serde_json::{Map, Value, json};
use std::collections::HashMap;

type Obj = Map<String, Value>;

fn as_obj(v: &Value) -> &Obj {
    static EMPTY: std::sync::LazyLock<Obj> = std::sync::LazyLock::new(Map::new);
    v.as_object().unwrap_or(&EMPTY)
}

fn num_add(a: &Value, b: &Value) -> Value {
    match (
        a.as_i64().filter(|_| !a.is_f64()),
        b.as_i64().filter(|_| !b.is_f64()),
    ) {
        (Some(x), Some(y)) => json!(x + y),
        _ => values::float(values::number(a).unwrap_or(0.0) + values::number(b).unwrap_or(0.0)),
    }
}

/// `x or 0`
fn or0(v: &Value) -> Value {
    if values::enabled(v) {
        v.clone()
    } else {
        json!(0)
    }
}

fn fnum(v: &Value) -> f64 {
    values::number(v).unwrap_or(0.0)
}

/// 站长点过「没问题」打的标签：24 小时内可确认的提醒不再算「要处理」。
const ACK_TAG: &str = "已查看";
const DAY_SECS: f64 = 86400.0;

/// 与 `web/src/pages/admin/sections/members/logic.ts` 的 `acked` 一致。
fn acked_24h(m: &Obj, now: f64) -> bool {
    m.get("tags").and_then(Value::as_array).is_some_and(|tags| {
        tags.iter().any(|t| {
            t.get("tag").and_then(Value::as_str) == Some(ACK_TAG)
                && now - t.get("ts").and_then(Value::as_f64).unwrap_or(0.0) < DAY_SECS
        })
    })
}

/// 「需要站长处理」：与 `web/src/pages/admin/sections/members/logic.ts` 的 `needsAttention` 同口径
/// （两处要一起改）。bad（限流暂停中 / 已过期）点过「没问题」也不豁免；其余（3 天内到期、
/// 自动标签刷屏、疑似/待查/观察标签、今日被重置 ≥2 次）在「已查看」后 24 小时内不算。
/// 停用的 Key 前端列表不显示，也不计入。
fn attention_needed(m: &Obj, now: f64) -> bool {
    if !m.get("enabled").and_then(Value::as_bool).unwrap_or(false) {
        return false;
    }
    let paused = m
        .get("share")
        .and_then(|s| s.get("paused_until"))
        .and_then(Value::as_f64)
        .is_some_and(|until| until > now);
    let expires_at = m.get("expires_at").and_then(Value::as_f64);
    let expired = expires_at.is_some_and(|at| at < now);
    if paused || expired {
        return true;
    }
    if acked_24h(m, now) {
        return false;
    }
    // 3 天内到期
    if expires_at.is_some_and(|at| at - now < 3.0 * DAY_SECS) {
        return true;
    }
    // 自动标签：单纯客户端短时连点「请求过快」不升级；有备注或近 24h ≥ 50 次才要处理
    let loud_auto_tag = m
        .get("auto_tags")
        .and_then(Value::as_array)
        .is_some_and(|tags| {
            tags.iter().any(|t| {
                let tag = t.get("tag").and_then(Value::as_str).unwrap_or("");
                let count = t.get("count").and_then(Value::as_i64).unwrap_or(0);
                let noted = t
                    .get("note")
                    .and_then(Value::as_str)
                    .is_some_and(|n| !n.is_empty());
                tag != "请求过快" && (noted || count >= 50)
            })
        });
    if loud_auto_tag {
        return true;
    }
    // 站长手动打的「疑似 / 待查 / 观察」标签（「已查看」不算）
    let flagged = m.get("tags").and_then(Value::as_array).is_some_and(|tags| {
        tags.iter().any(|t| {
            let tag = t.get("tag").and_then(Value::as_str).unwrap_or("");
            tag != ACK_TAG && ["疑似", "待查", "观察"].iter().any(|w| tag.contains(w))
        })
    });
    if flagged {
        return true;
    }
    // 今天被手动重置额度 ≥ 2 次
    m.get("reset_today")
        .and_then(|r| r.get("count"))
        .and_then(Value::as_i64)
        .is_some_and(|n| n >= 2)
}

/// 每位成员（Key）的今日 / 近 7 天 / 累计用量，以及来源。
///
/// `?filter=all|attention`（缺省 = `all`，完全旧行为）：`attention` 只返回需要站长处理的行，
/// 同时给出 `total`（裁剪前成员行数）/ `attention`（需要处理的行数）/ `filter`（实际生效值）。
/// 前端 `web/src/pages/admin/sections/members/logic.ts` 的 `needsAttention` 与本文件的
/// [`attention_needed`] 是同一条口径，改一边要同时改另一边。
pub(super) async fn members_view(state: &AppState, filter_raw: &str) -> Result<Value, ApiError> {
    let policies = state.db.member_policy_directory().await?;
    let filter = match filter_raw {
        "" | "all" => "all",
        "attention" => "attention",
        _ => return Err(http(422, "filter 只能是 all 或 attention")),
    };
    let st = state.clone();
    let now = st.now();
    let today = st.day();
    let week_start = st.clock.last_days(now, 7).remove(0);
    let week: HashMap<i64, Value> = st.db.member_usage(&week_start).await?.into_iter().collect();
    let totals: HashMap<i64, i64> = st
        .db
        .generated_image_totals(None)
        .await?
        .into_iter()
        .collect();
    let reg: HashMap<i64, RegistrationRow> = st
        .db
        .registration_rows()
        .await?
        .into_iter()
        .map(|r| (r.key_id, r))
        .collect();
    let since = now - 24.0 * 3600.0;
    let milestones: HashMap<i64, _> = st
        .db
        .member_milestones(since, &|c, d| gate_core::reasons::reason_label(c, d))
        .await?
        .into_iter()
        .collect();
    // 暂停已过期的输出整数 0，没过期的输出浮点剩余秒数
    let pause_map: HashMap<i64, Value> = st
        .db
        .pause_rows()
        .await?
        .into_iter()
        .map(|(k, paused)| {
            let v = if paused > now {
                values::float(paused)
            } else {
                json!(0)
            };
            (k, v)
        })
        .collect();

    // 标签：手动（key_tags，站长 / 运维打的）+ 自动（since 之后因已知的成员侧原因被拒 / 失败）
    let mut manual_tags: HashMap<i64, Vec<TagRow>> = HashMap::new();
    for t in st.db.key_tag_rows().await? {
        manual_tags.entry(t.key_id).or_default().push(t);
    }
    let mut auto_tags: HashMap<i64, Vec<Value>> = HashMap::new();
    for (kid, label, n) in st.db.auto_tag_counts(since, &AUTO_TAG_PATTERNS).await? {
        auto_tags
            .entry(kid)
            .or_default()
            .push(json!({"tag": label, "count": n}));
    }

    // 今日「重置今日额度」痕迹：按服务日起点统计次数与最近一次（从操作日志取，只读）。
    // 起点是**当前服务日**的 7 点（不能用日历日期算，凌晨 0–7 点起点会落在未来）。
    let day_start = st.clock.service_day_start(&today).unwrap_or(0.0);
    let mut reset_map: HashMap<i64, (i64, f64)> = HashMap::new();
    for (a_ts, target) in st.db.quota_reset_marks(day_start).await? {
        let Some(rest) = target.strip_prefix("Key #") else {
            continue;
        };
        let digits: String = rest.chars().take_while(|c| validation::is_nd(*c)).collect();
        let Some(id) = primitives::parse_int(&digits) else {
            continue;
        };
        let e = reset_map.entry(id).or_insert((0, 0.0));
        e.0 += 1;
        if a_ts > e.1 {
            e.1 = a_ts;
        }
    }
    let today_counters = st.db.counter_map(&today).await?;
    let mut out: Vec<Obj> = Vec::new();
    for row in st.db.list_keys().await? {
        let id = row.get("id").and_then(Value::as_i64).unwrap_or(0);
        let counter = today_counters.get(&id).cloned().unwrap_or_default();
        let (images, v5, anlas, requests) =
            (counter.images, counter.v5, counter.anlas, counter.requests);
        let w = week.get(&id).cloned().unwrap_or(Value::Null);
        let wi = |k: &str| values::number(&jget(&w, k)).map_or(0, |x| x as i64);
        let wf = |k: &str| values::number(&jget(&w, k)).unwrap_or(0.0);
        let my_tags = manual_tags.get(&id);
        let invited = my_tags.is_some_and(|t| t.iter().any(|x| x.tag == INVITE_TAG));
        let reg_row = reg.get(&id);
        let ms = milestones.get(&id);
        let mut m = Obj::new();
        let mut put = |k: &str, v: Value| {
            m.insert(k.to_string(), v);
        };
        put("id", jget(&row, "id"));
        put("name", jget(&row, "name"));
        put("enabled", json!(jflag(&row, "enabled")));
        put("invited", json!(invited));
        put("discord_id", json!(reg_row.map(|r| r.discord_id.clone())));
        put("discord", json!(reg_row.map(discord_profile)));
        put("created_at", jget(&row, "created_at"));
        put("last_used_at", jget(&row, "last_used_at"));
        put("expires_at", jget(&row, "expires_at"));
        put(
            "policy",
            serde_json::to_value(
                policies.for_subject(row.get("discord_id").and_then(Value::as_str))?,
            )
            .map_err(|_| crate::error::InternalError("成员规则编码失败".into()))?,
        );
        put(
            "today",
            json!({"images": images, "legacy_free_images":counter.legacy_free_images,"v5": v5, "anlas": values::float(primitives::round(anlas, 2)),
                   "requests": requests}),
        );
        put(
            "week",
            json!({"images": wi("images"), "v5": wi("v5"),
                   "anlas": values::float(primitives::round(wf("anlas"), 2)), "requests": wi("requests")}),
        );
        put("total_images", json!(totals.get(&id).copied().unwrap_or(0)));
        // 筛选用：领取时间（Discord 领取优先，否则建 Key 时间）、首次 / 最近成功出图、24h 被拒次数
        put(
            "registered_at",
            match reg_row {
                Some(r) => json!(r.created_at),
                None => jget(&row, "created_at"),
            },
        );
        put("first_image_at", json!(ms.and_then(|m| m.first_image_at)));
        put("last_image_at", json!(ms.and_then(|m| m.last_image_at)));
        put("rejected_24h", json!(ms.map_or(0, |m| m.rejected_24h)));
        put(
            "rejected_why",
            json!(ms.and_then(|m| m.rejected_why.clone()).unwrap_or_default()),
        );
        // 字段名沿用 2.x 的 "share"（前端和 admin_snapshot 按这个名字读）；内容只剩 Key 临时暂停的到期时间
        put(
            "share",
            pause_map
                .get(&id)
                .map_or(Value::Null, |p| json!({"paused_until": p})),
        );
        put(
            "reset_today",
            reset_map.get(&id).map_or(
                Value::Null,
                |(c, t)| json!({"count": c, "last_ts": values::float(*t)}),
            ),
        );
        put(
            "tags",
            Value::Array(
                my_tags
                    .into_iter()
                    .flatten()
                    .map(|t| json!({"tag": t.tag, "note": t.note, "by": t.by, "ts": values::float(t.ts)}))
                    .collect(),
            ),
        );
        put(
            "auto_tags",
            Value::Array(auto_tags.get(&id).cloned().unwrap_or_default()),
        );
        out.push(m);
    }

    // 成员为中心合并（10-14 站长）：同一 Discord 成员重置 / 换 Key 留下的多把 Key 归并成一条，
    // 用量与统计跨 Key 汇总；key_count / all_key_ids 供前端展示与跨 Key 查生成记录。
    let mut order: Vec<String> = Vec::new();
    let mut by_discord: HashMap<String, Vec<Obj>> = HashMap::new();
    let mut singles: Vec<Obj> = Vec::new();
    for e in out {
        match e
            .get("discord_id")
            .and_then(Value::as_str)
            .map(str::to_string)
        {
            None => {
                // 没有 Discord 绑定的 Key（站长手建 / 邀请 Key）也要带 key_count / all_key_ids：
                // 前端成员列表与「生成记录」按这两个字段取数，缺了会 TypeError（2026-10-17 审查）。
                let mut e = e;
                let id = e["id"].clone();
                e.insert("key_count".into(), json!(1));
                e.insert("all_key_ids".into(), json!([id]));
                singles.push(e);
            }
            Some(did) => {
                if !by_discord.contains_key(&did) {
                    order.push(did.clone());
                }
                by_discord.entry(did).or_default().push(e);
            }
        }
    }
    let mut merged: Vec<Obj> = Vec::new();
    for did in order {
        let mut entries = by_discord.remove(&did).unwrap_or_default();
        if entries.len() == 1 {
            let mut e = entries.remove(0);
            let id = e["id"].clone();
            e.insert("key_count".into(), json!(1));
            e.insert("all_key_ids".into(), json!([id]));
            singles.push(e);
            continue;
        }
        // 已启用的在前，同组里最近用过的在前（稳定排序）
        entries.sort_by(|a, b| {
            let key = |e: &Obj| {
                (
                    !e["enabled"].as_bool().unwrap_or(false),
                    -fnum(&or0(&e["last_used_at"])),
                )
            };
            let (ka, kb) = (key(a), key(b));
            ka.0.cmp(&kb.0)
                .then(ka.1.partial_cmp(&kb.1).unwrap_or(std::cmp::Ordering::Equal))
        });
        let mut head = entries[0].clone();
        head.insert("key_count".into(), json!(entries.len()));
        head.insert(
            "all_key_ids".into(),
            Value::Array(entries.iter().map(|e| e["id"].clone()).collect()),
        );
        for other in &entries[1..] {
            let enabled = head["enabled"].as_bool().unwrap_or(false)
                || other["enabled"].as_bool().unwrap_or(false);
            head.insert("enabled".into(), json!(enabled));
            for f in ["today", "week"] {
                let mut h = as_obj(&head[f]).clone();
                for (k, v) in as_obj(&other[f]) {
                    if v.is_number() && h.get(k).is_some_and(Value::is_number) {
                        let sum = num_add(&h[k], v);
                        h.insert(k.clone(), sum);
                    }
                }
                head.insert(f.into(), Value::Object(h));
            }
            head.insert(
                "total_images".into(),
                num_add(&head["total_images"], &other["total_images"]),
            );
            head.insert(
                "rejected_24h".into(),
                num_add(&head["rejected_24h"], &other["rejected_24h"]),
            );
            // max(a or 0, b or 0)：b 严格更大才取 b
            let (a, b) = (or0(&head["last_used_at"]), or0(&other["last_used_at"]));
            head.insert(
                "last_used_at".into(),
                if fnum(&b) > fnum(&a) { b } else { a },
            );
            if values::enabled(&other["last_image_at"]) {
                let a = or0(&head["last_image_at"]);
                let b = other["last_image_at"].clone();
                head.insert(
                    "last_image_at".into(),
                    if fnum(&b) > fnum(&a) { b } else { a },
                );
            }
            if values::enabled(&other["first_image_at"]) {
                let b = other["first_image_at"].clone();
                let a = if values::enabled(&head["first_image_at"]) {
                    head["first_image_at"].clone()
                } else {
                    b.clone()
                };
                head.insert(
                    "first_image_at".into(),
                    if fnum(&b) < fnum(&a) { b } else { a },
                );
            }
            let seen: Vec<Value> = head["tags"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|t| t["tag"].clone())
                .collect();
            let mut tags: Vec<Value> = head["tags"].as_array().cloned().unwrap_or_default();
            tags.extend(
                other["tags"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|t| !seen.contains(&t["tag"]))
                    .cloned(),
            );
            head.insert("tags".into(), Value::Array(tags));
        }
        merged.push(head);
    }
    singles.extend(merged);
    // 需要站长处理的行：口径与后台前端 logic.ts 的 needsAttention 一致（含「已查看」24 小时豁免；
    // 停用的 Key 前端列表本来就不显示，也不计入）。`total` 是裁剪前的成员行数。
    let total = singles.len();
    let attention = singles.iter().filter(|m| attention_needed(m, now)).count();
    let members: Vec<Obj> = if filter == "attention" {
        singles
            .into_iter()
            .filter(|m| attention_needed(m, now))
            .collect()
    } else {
        singles
    };
    Ok(json!({
        "members": members,
        "inactivity_days": st.env.key_inactivity_delete_days,
        "total": total,
        "attention": attention,
        "filter": filter,
    }))
}
