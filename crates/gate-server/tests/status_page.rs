//! 公开状态页（事件直接写库，没有后台接口）。
//!
//! 快照 `tests/snapshots/status/scenarios.jsonl`：每行一个场景（合成事件 / 小时计数 / 上游请求 / 官方状态），
//! 输出是 `/status.json` 的内容，以及 `/status` 页面的
//! 可见文本（文本节点按顺序，加上 title / aria-label / href / role / lang 属性；样式 / 脚本的内容和带内容哈希的资源地址不存，
//! 页面样式改了不会让快照失效，文案改了会）。
//! 更新：`UPDATE_SNAPSHOT=1 cargo test -p gate-server --test status_page`，然后审阅 `git diff`。
//!
//! 时钟是全进程共享的（`time::freeze`），所以每个测试先拿 [`clock`] 的锁。

mod common;
use common::*;
use gate_snapshot::snapshot::{Suite, path as snapshot_path};
use serde_json::{Map, Value, json};

const M: &str = env!("CARGO_MANIFEST_DIR");

/// 2026-10-14 12:00:00 +08:00
const NOW: f64 = 1_791_950_400.0;

/// 快照里某个场景的输入（只读文件，不比较输出）。
fn scenario_input(name: &str) -> Value {
    let path = snapshot_path(M, "status/scenarios");
    std::fs::read_to_string(&path)
        .unwrap()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str::<Value>(l).unwrap()["in"].clone())
        .find(|sc| sc["scenario"] == name)
        .unwrap_or_else(|| panic!("快照里没有场景 {name}"))
}

fn text(body: &[u8]) -> String {
    String::from_utf8(body.to_vec()).unwrap()
}

async fn get(e: &Env, path: &str) -> (u16, axum::http::HeaderMap, Vec<u8>) {
    let (s, h, b) = Call::new("GET", path).send(&e.state).await;
    (s.as_u16(), h, b)
}

/// 把夹具里的输入写进库：事件走 gate-db 的写入接口，其余直接落表。
async fn load_inputs(e: &Env, sc: &Value) {
    for spec in sc["incidents"].as_array().unwrap() {
        let comps: Vec<String> = spec["components"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c.as_str().unwrap().to_string())
            .collect();
        let ups = spec["updates"].as_array().unwrap();
        let first = &ups[0];
        let id = e
            .state
            .db
            .create_incident(
                spec["title"].as_str().unwrap(),
                spec["impact"].as_str().unwrap(),
                &comps,
                first[0].as_str().unwrap(),
                first[1].as_str().unwrap(),
                first[2].as_f64().unwrap(),
            )
            .await
            .unwrap();
        for u in &ups[1..] {
            let ok = e
                .state
                .db
                .add_incident_update(
                    id,
                    u[0].as_str().unwrap(),
                    u[1].as_str().unwrap(),
                    u[2].as_f64().unwrap(),
                )
                .await
                .unwrap();
            assert!(ok);
        }
    }
    let mut sql = String::new();
    for r in sc["status_hourly"].as_array().unwrap() {
        sql += &format!(
            "INSERT INTO status_hourly(hour, code, n) VALUES ({}, {}, {});\n",
            r[0].as_i64().unwrap(),
            r[1].as_i64().unwrap(),
            r[2].as_i64().unwrap()
        );
    }
    for r in sc["usage"].as_array().unwrap() {
        let up = r[2].as_i64().unwrap();
        sql += &format!(
            "INSERT INTO usage_log(ts, key_id, kind, status, up_status) VALUES ({:?}, {}, 'image', '{}', {up});\n",
            r[0].as_f64().unwrap(),
            r[1].as_i64().unwrap(),
            if up == 200 { "ok" } else { "error" },
        );
    }
    insert_sql(e, sql).await;
    if let Some(off) = sc["official"].as_object() {
        e.state
            .db
            .set_setting(
                "novelai_official_status",
                &serde_json::to_string(off).unwrap(),
            )
            .await
            .unwrap();
    }
}

async fn insert_sql(e: &Env, sql: String) {
    if sql.is_empty() {
        return;
    }
    e.state
        .db
        .write(move |tx| tx.execute_batch(&sql))
        .await
        .unwrap();
}

/// 合成上游请求：`ok` 个成功（来自 Key 1–5 轮流）+ 若干 `(up_status, key_id)` 失败，都在最近一小时内。
async fn add_usage(e: &Env, ok: usize, failures: &[(i64, i64)]) {
    let mut sql = String::new();
    let base = NOW - 3000.0;
    for i in 0..ok {
        sql += &format!(
            "INSERT INTO usage_log(ts, key_id, kind, status, up_status) VALUES ({:?}, {}, 'image', 'ok', 200);\n",
            base + i as f64 * 20.0,
            (i % 5) + 1
        );
    }
    for (j, (code, key)) in failures.iter().enumerate() {
        sql += &format!(
            "INSERT INTO usage_log(ts, key_id, kind, status, up_status) VALUES ({:?}, {key}, 'image', 'error', {code});\n",
            base + 2000.0 + j as f64
        );
    }
    insert_sql(e, sql).await;
}

/// HTML 实体还原（页面由 maud 渲染，只会出现这几种）。
fn unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
        .replace("&amp;", "&")
}

/// 要记录的属性。
const KEPT_ATTRS: [&str; 5] = ["title", "aria-label", "href", "role", "lang"];

/// 解析一个开始标签里的属性（`name="value"`；没有值的属性忽略）。
fn tag_attrs(tag: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = tag;
    while let Some(eq) = rest.find("=\"") {
        let name_start = rest[..eq]
            .rfind(|c: char| c.is_whitespace() || c == '<')
            .map_or(0, |i| i + 1);
        let name = rest[name_start..eq].to_string();
        let after = &rest[eq + 2..];
        let end = after.find('"').unwrap_or(after.len());
        out.push((name, unescape(&after[..end])));
        rest = &after[(end + 1).min(after.len())..];
    }
    out
}

/// 页面的可见文本：`{"texts": [文本节点…], "attrs": [[属性名, 值]…]}`，都按文档顺序。
/// `<script>` / `<style>` 的内容不算；`/_owl/` 资源地址里的内容哈希（`?v=…`）去掉。
fn visible(html: &str) -> Value {
    let (mut texts, mut attrs) = (Vec::new(), Vec::new());
    let mut rest = html;
    while !rest.is_empty() {
        let Some(lt) = rest.find('<') else {
            push_text(&mut texts, rest);
            break;
        };
        push_text(&mut texts, &rest[..lt]);
        rest = &rest[lt..];
        if let Some(body) = rest.strip_prefix("<!--") {
            rest = body.find("-->").map_or("", |i| &body[i + 3..]);
            continue;
        }
        let end = rest.find('>').map_or(rest.len(), |i| i + 1);
        let tag = &rest[..end];
        rest = &rest[end..];
        let name = tag[1..]
            .split(|c: char| c.is_whitespace() || c == '>' || c == '/')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        for (k, v) in tag_attrs(tag) {
            if KEPT_ATTRS.contains(&k.as_str()) {
                let v = match v.split_once("?v=") {
                    Some((path, _)) if path.starts_with("/_owl/") => path.to_string(),
                    _ => v,
                };
                attrs.push(json!([k, v]));
            }
        }
        if name == "script" || name == "style" {
            let close = format!("</{name}");
            rest = rest.find(&close).map_or("", |i| &rest[i..]);
        }
    }
    json!({"texts": texts, "attrs": attrs})
}

fn push_text(texts: &mut Vec<String>, raw: &str) {
    let t = unescape(raw.trim());
    if !t.is_empty() {
        texts.push(t);
    }
}

/// 紧凑 JSON 的响应存成 JSON 值（读 diff 方便），不是紧凑 JSON 的存原文字符串：逐字节一致。
fn json_or_text(body: &str) -> Value {
    match serde_json::from_str::<Value>(body) {
        Ok(v) if serde_json::to_string(&v).is_ok_and(|compact| compact == body) => v,
        _ => Value::String(body.to_string()),
    }
}

#[tokio::test]
async fn scenario_outputs_match_snapshot() {
    let _g = clock(None).await;
    let mut suite = Suite::open(M, "status/scenarios");
    let cases = suite.cases();
    for (i, case) in cases.iter().enumerate() {
        let sc = &case.raw;
        let name = sc["scenario"].as_str().unwrap().to_string();
        gate_server::time::freeze(Some(sc["now"].as_f64().unwrap()));
        let discord = &sc["discord"];
        let (guild, chan) = (
            discord["guild"].as_str().unwrap().to_string(),
            discord["channel"].as_str().unwrap().to_string(),
        );
        let extra: Vec<(&str, &str)> = if guild.is_empty() {
            vec![]
        } else {
            vec![("DISCORD_GUILD_ID", &guild), ("ANNOUNCE_CHANNEL_ID", &chan)]
        };
        let e = setup(&extra).await;
        load_inputs(&e, sc).await;
        let mut out = Map::new();

        // components 列的原文
        let cols: Vec<String> = e
            .state
            .db
            .read(|c| {
                let mut st = c.prepare("SELECT components FROM incidents ORDER BY id")?;
                st.query_map([], |r| r.get::<_, String>(0))?.collect()
            })
            .await
            .unwrap();
        out.insert("components_column".into(), json!(cols));

        // JSON
        let (s, h, b) = get(&e, "/status.json").await;
        assert_eq!(s, 200, "{name}");
        assert_eq!(h["content-type"], "application/json", "{name}");
        assert_eq!(h["cache-control"], "no-cache", "{name}");
        out.insert("status_json".into(), json_or_text(&text(&b)));

        // HTML
        let (s, h, b) = get(&e, "/status").await;
        assert_eq!(s, 200, "{name}");
        assert_eq!(h["content-type"], "text/html; charset=utf-8", "{name}");
        out.insert("html".into(), visible(&text(&b)));
        suite.record(i, Some(Value::Object(out)));
    }
    gate_server::time::freeze(None);
    // 场景没被意外删掉
    assert!(cases.len() >= 12, "状态页快照只剩 {} 个场景", cases.len());
    suite.finish();
}

#[tokio::test]
async fn html_is_hardened_and_links_work() {
    let _g = clock(Some(NOW)).await;
    let e = setup(&[]).await;
    load_inputs(&e, &scenario_input("history_mix")).await;
    let (s, h, b) = get(&e, "/status").await;
    assert_eq!(s, 200);
    let html = text(&b);

    // 安全头
    let csp = h["content-security-policy"].to_str().unwrap();
    assert!(
        !csp.contains("unsafe-inline") && !csp.contains("unsafe-eval"),
        "{csp}"
    );
    for need in [
        "default-src 'none'",
        "script-src 'self'",
        "style-src 'self'",
        "frame-ancestors 'none'",
        "base-uri 'none'",
        "form-action 'none'",
    ] {
        assert!(csp.contains(need), "CSP 缺 {need}：{csp}");
    }
    assert_eq!(h["x-frame-options"], "DENY");
    assert_eq!(h["x-content-type-options"], "nosniff");
    assert_eq!(h["referrer-policy"], "no-referrer");
    assert_eq!(h["cache-control"], "no-cache");
    assert_eq!(
        h["content-length"]
            .to_str()
            .unwrap()
            .parse::<usize>()
            .unwrap(),
        b.len()
    );

    // 页面里没有内联样式 / 脚本 / 事件处理器
    assert!(!html.contains(" style="), "不应有 style 属性");
    assert!(!html.contains("<style"), "不应有 <style>");
    assert!(!html.contains(" onclick=") && !html.contains("javascript:"));
    for chunk in html.split("<script").skip(1) {
        let tag = chunk.split('>').next().unwrap();
        assert!(
            tag.contains("src=\"/_owl/status-page.js?v="),
            "内联脚本：{tag}"
        );
        assert!(
            chunk.starts_with(&format!("{tag}></script>")),
            "脚本标签里不应有内容"
        );
    }

    // 过去事件的标题是真正的链接，指向同页锚点；锚点存在
    let links: Vec<&str> = html
        .split("<a class=\"sp-inc-link\" href=\"")
        .skip(1)
        .map(|r| r.split('"').next().unwrap())
        .collect();
    assert!(links.len() >= 10, "过去事件链接太少：{links:?}");
    for l in links {
        assert!(l.starts_with("#incident-"), "{l}");
        assert!(
            html.contains(&format!("id=\"{}\"", &l[1..])),
            "缺少锚点 {l}"
        );
    }
    // 和首页同一个顶栏：Logo 回首页；状态页路由仍可直达，顶栏不重复展示当前页按钮。
    assert!(html.contains("<a class=\"brand\" href=\"/\">"));
    assert!(!html.contains("<nav class=\"site-nav\""));
    assert!(!html.contains("捐赠"));
    // 色条的颜色走类名，不写 fill
    assert!(!html.contains(" fill=\"#"), "色条不应写死颜色");
    assert!(html.contains("<rect class=\"lv-0\""));

    // 样式与脚本由本站提供，地址带内容哈希，长期缓存
    let css_href = html
        .split("<link rel=\"stylesheet\" href=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap()
        .to_string();
    assert!(
        css_href.starts_with("/_owl/status-page.css?v="),
        "{css_href}"
    );
    let (s, h, b) = get(&e, &css_href).await;
    assert_eq!(s, 200);
    assert_eq!(h["content-type"], "text/css; charset=utf-8");
    assert_eq!(h["cache-control"], "public, max-age=31536000, immutable");
    assert_eq!(h["x-content-type-options"], "nosniff");
    // 首页的设计令牌拼在同一份样式表里
    assert!(text(&b).contains("--bg: #f9f9f8;"));
    assert!(text(&b).contains(".site-top"));
    let js_href = html
        .split("<script src=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap()
        .to_string();
    let (s, h, b) = get(&e, &js_href).await;
    assert_eq!(s, 200);
    assert_eq!(h["content-type"], "text/javascript; charset=utf-8");
    assert_eq!(h["cache-control"], "public, max-age=31536000, immutable");
    assert!(text(&b).contains("owl-theme"));
    // 地址里的哈希对不上（旧页面缓存）：不长期缓存
    let (s, h, _) = get(&e, "/_owl/status-page.css?v=stale").await;
    assert_eq!((s, h["cache-control"].to_str().unwrap()), (200, "no-cache"));
    // 其它 /_owl/ 资源仍走原来的静态资源处理（测试里没有前端构建产物 → 404）
    let (s, _, _) = get(&e, "/_owl/no-such-file.js").await;
    assert_eq!(s, 404);
}

#[tokio::test]
async fn head_returns_headers_without_body() {
    let _g = clock(Some(NOW)).await;
    let e = setup(&[]).await;
    let (s, h, b) = get(&e, "/status").await;
    assert_eq!(s, 200);
    let (hs, hh, hb) = Call::new("HEAD", "/status").send(&e.state).await;
    assert_eq!(hs, 200);
    assert!(hb.is_empty());
    assert_eq!(hh["content-type"], h["content-type"]);
    assert_eq!(hh["content-security-policy"], h["content-security-policy"]);
    assert_eq!(hh["content-length"], h["content-length"]);
    assert_eq!(
        hh["content-length"]
            .to_str()
            .unwrap()
            .parse::<usize>()
            .unwrap(),
        b.len()
    );
    // 方法不对
    let (s, _, _) = Call::new("POST", "/status").send(&e.state).await;
    assert_eq!(s, 405);
}

/// 3.0 新规则：「现在」的上游状态要求失败来自至少 2 把 Key 且至少 3 次；每天的色条仍按旧版的占比。
#[tokio::test]
async fn upstream_now_needs_failures_from_two_keys() {
    let _g = clock(Some(NOW)).await;
    let status_of = |body: &[u8]| -> (String, String) {
        let v: Value = serde_json::from_slice(body).unwrap();
        let s = |i: usize| v["components"][i]["status"].as_str().unwrap().to_string();
        (s(0), s(1))
    };

    // 同一把 Key 失败 5 次、100 次成功 → 运行正常
    let e = setup(&[]).await;
    add_usage(&e, 100, &[(500, 4); 5]).await;
    let (_, _, b) = get(&e, "/status.json").await;
    assert_eq!(status_of(&b), ("运行正常".into(), "运行正常".into()));
    let (_, _, h) = get(&e, "/status").await;
    let html = text(&h);
    assert!(html.contains("所有系统运行正常"));
    // 今天的色条仍然按当天占比（4.8% > 1%）变黄
    let bars = html
        .split("<div class=\"sp-bars\">")
        .nth(1)
        .unwrap()
        .split("</svg>")
        .next()
        .unwrap();
    let last_rect = bars.rsplit("<rect").next().unwrap();
    assert!(last_rect.contains("class=\"lv-1\""), "{last_rect}");

    // 两把 Key 各失败 → 性能下降（页首横幅也变）
    let e = setup(&[]).await;
    add_usage(&e, 100, &[(500, 1), (500, 1), (429, 2)]).await;
    let (_, _, b) = get(&e, "/status.json").await;
    assert_eq!(status_of(&b), ("性能下降".into(), "运行正常".into()));
    let (_, _, h) = get(&e, "/status").await;
    assert!(text(&h).contains("部分系统性能下降"));

    // 两把 Key 但只失败 2 次 → 不算
    let e = setup(&[]).await;
    add_usage(&e, 100, &[(500, 1), (500, 2)]).await;
    let (_, _, b) = get(&e, "/status.json").await;
    assert_eq!(status_of(&b), ("运行正常".into(), "运行正常".into()));

    // 请求不到 10 次不下结论（与旧版一致）
    let e = setup(&[]).await;
    add_usage(&e, 3, &[(500, 1), (500, 2), (500, 3)]).await;
    let (_, _, b) = get(&e, "/status.json").await;
    assert_eq!(status_of(&b), ("运行正常".into(), "运行正常".into()));

    // 官方状态与进行中的事件照旧生效（不受新规则影响）
    let e = setup(&[]).await;
    e.state
        .db
        .set_setting(
            "novelai_official_status",
            &format!(r#"{{"image_code": 300, "fetched_at": {NOW}}}"#),
        )
        .await
        .unwrap();
    let (_, _, b) = get(&e, "/status.json").await;
    assert_eq!(status_of(&b), ("性能下降".into(), "运行正常".into()));
}

/// 快照缓存 60 秒：期间新发布的事件还看不到（与旧版一致），过了就看到。
#[tokio::test]
async fn snapshot_is_cached_for_a_minute() {
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    let _g = clock(Some(NOW)).await;
    let e = setup(&[]).await;
    // 缓存挂在路由上：同一个 app 才共享（线上 app() 只组装一次）
    let app = gate_server::app(e.state.clone());
    macro_rules! fetch {
        () => {{
            let req = axum::http::Request::builder()
                .uri("/status.json")
                .body(axum::body::Body::empty())
                .unwrap();
            let resp = app.clone().oneshot(req).await.unwrap();
            let bytes = resp.into_body().collect().await.unwrap().to_bytes();
            serde_json::from_slice::<Value>(&bytes).unwrap()
        }};
    }
    assert_eq!(fetch!()["incidents"], json!([]));
    e.state
        .db
        .create_incident(
            "出图接口请求超时",
            "minor",
            &["api".to_string()],
            "investigating",
            "x",
            NOW,
        )
        .await
        .unwrap();
    gate_server::time::freeze(Some(NOW + 30.0));
    assert_eq!(fetch!()["incidents"], json!([]), "60 秒内复用缓存");
    gate_server::time::freeze(Some(NOW + 61.0));
    let v = fetch!();
    assert_eq!(v["incidents"][0]["title"], "出图接口请求超时");
    assert_eq!(v["active"].as_array().unwrap().len(), 1);
    gate_server::time::freeze(None);
}

/// 直接写库的事件（标准句式正文）在公开页上看得到。
#[tokio::test]
async fn published_incident_shows_on_public_pages() {
    let _g = clock(Some(NOW)).await;
    let e = setup(&[]).await;
    e.state
        .db
        .create_incident(
            "出图接口请求超时",
            "major",
            &["api".to_string()],
            "investigating",
            "我们正在调查此问题。",
            NOW - 120.0,
        )
        .await
        .unwrap();
    let (_, _, b) = get(&e, "/status").await;
    let html = text(&b);
    assert!(html.contains("出图接口请求超时") && html.contains("我们正在调查此问题。"));
    assert!(html.contains("2026年10月14日 11:58（北京时间）"));
    assert!(html.contains("部分中断"));
    // 有未解决事件时页首是事件条而不是「所有系统运行正常」
    assert!(!html.contains("所有系统运行正常"));
}

/// 同一天、同标题、非重大（minor / none）的自动维护事件合成一条，显示次数与起止；
/// 人工重大事件（major / critical）单独保留。
#[tokio::test]
async fn same_day_routine_incidents_are_merged_on_page() {
    let _g = clock(Some(NOW)).await;
    let e = setup(&[]).await;
    for (i, at) in [NOW - 3600.0, NOW - 3000.0, NOW - 2400.0]
        .iter()
        .enumerate()
    {
        e.state
            .db
            .create_incident(
                "出图接口暂时无法访问",
                "minor",
                &["api".to_string()],
                "resolved",
                &format!("此事件已解决（第 {i} 次）。"),
                *at,
            )
            .await
            .unwrap();
    }
    e.state
        .db
        .create_incident(
            "上游中断（人工事件）",
            "major",
            &["api".to_string()],
            "resolved",
            "此事件已解决。",
            NOW - 1800.0,
        )
        .await
        .unwrap();
    let (_, _, b) = get(&e, "/status").await;
    let html = text(&b);
    // 三次自动维护事件只剩一张卡片（一条 sp-inc-link），人工重大事件单独一张
    assert_eq!(
        html.matches("class=\"sp-inc-link\"").count(),
        2,
        "同一天同标题应合并；页面高度不应再被重复事件撑开"
    );
    assert!(html.contains("共 3 次 · 11:00–11:20（北京时间）"), "{html}");
    // 合并组只展示最新一条进展，不再重复三遍几乎相同的正文
    assert!(!html.contains("第 0 次") && !html.contains("第 1 次"));
    assert!(html.contains("第 2 次"));
    // 被合并的旧事件保留锚点，旧深链接不会失效
    assert!(html.contains("id=\"incident-1\"") && html.contains("id=\"incident-2\""));
    // 人工重大事件保留完整卡片
    assert!(html.contains("上游中断（人工事件）"));
    // 状态页不再用 scroll-driven `.reveal`（blur + opacity 会让文字不可读、axe 判低对比）
    assert!(!html.contains("reveal"), "不应有 .reveal");
}

/// 过去的事件按天分组；默认只展开最近 7 天，更早的收进原生 `<details>`（无 JS 也能展开）。
#[tokio::test]
async fn older_history_is_folded_into_details() {
    let _g = clock(Some(NOW)).await;
    let e = setup(&[]).await;
    let (_, _, b) = get(&e, "/status").await;
    let html = text(&b);
    assert!(html.contains("<div class=\"sp-day\">"), "{html}");
    // 15 天都按天分组，最近 7 天默认可见，另外 8 天在折叠里
    assert_eq!(html.matches("class=\"sp-day\"").count(), 15);
    assert!(html.contains("<details class=\"sp-older\">"));
    assert!(html.contains("<summary>更早的事件（8 天）</summary>"));
    // 今天的小标题带「（今天）」
    assert!(html.contains("10月14日（今天）"));
}

/// 色条从第一个有数据的日子（2026-10-09）画起，不再是一排 75 天灰条；
/// 图例给出可读的日期范围与颜色含义（触屏没有 hover，title 仍保留）。
#[tokio::test]
async fn bars_start_at_first_tracked_day_and_legend_is_readable() {
    let _g = clock(Some(NOW)).await;
    let e = setup(&[]).await;
    let (_, _, b) = get(&e, "/status").await;
    let html = text(&b);
    // 2026-10-09 … 2026-10-14 共 6 根
    assert!(html.contains("10月9日 – 10月14日"), "{html}");
    let bars = html
        .split("<div class=\"sp-bars\">")
        .nth(1)
        .unwrap()
        .split("</svg>")
        .next()
        .unwrap();
    assert_eq!(bars.matches("<rect").count(), 6, "只画有数据的 6 天");
    // 触屏 / 读屏：整条色条有文字范围，每一天仍保留 title
    assert!(bars.contains("10月9日 – 10月14日 的运行状态色条"));
    assert!(bars.contains("<title>"));
    assert!(bars.contains("<title>"));
    // 颜色图例是可见文字
    assert!(html.contains("绿色正常"));
    // 不再出现「无数据」的灰条（开始记录之前的日子不画）
    assert!(!bars.contains("无数据"));
}

/// 「加入 Discord 获取更新」优先用邀请链接（匿名可加入），没有邀请再用公告频道；
/// 两者都没有时不放一个指回 /status 自己的假链接。
#[tokio::test]
async fn subscribe_prefers_discord_invite() {
    let _g = clock(Some(NOW)).await;

    let e = setup(&[
        ("DISCORD_INVITE_URL", "https://discord.gg/owl"),
        ("DISCORD_GUILD_ID", "1"),
        ("ANNOUNCE_CHANNEL_ID", "2"),
    ])
    .await;
    let (_, _, b) = get(&e, "/status").await;
    let html = text(&b);
    assert!(html.contains("加入 Discord 获取更新"));
    assert!(
        html.contains("href=\"https://discord.gg/owl\""),
        "应优先用邀请链接"
    );
    assert!(!html.contains("discord.com/channels/"), "不要再用频道链接");

    // 没配邀请：退回公告频道（老行为），文案也用新说法
    let e = setup(&[("DISCORD_GUILD_ID", "1"), ("ANNOUNCE_CHANNEL_ID", "2")]).await;
    let (_, _, b) = get(&e, "/status").await;
    let html = text(&b);
    assert!(html.contains("加入 Discord 获取更新"));
    assert!(html.contains("href=\"https://discord.com/channels/1/2\""));

    // 两个都没配：不放按钮
    let e = setup(&[]).await;
    let (_, _, b) = get(&e, "/status").await;
    let html = text(&b);
    assert!(!html.contains("class=\"btn sp-sub\""), "{html}");
}
