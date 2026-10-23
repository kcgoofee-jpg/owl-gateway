//! 后台「日志与记录」接口：用量日志 / 操作日志 / Bug / 生成记录 / 反馈 / 问卷 / 捐赠 / 全站重置。
//!
//! 逐字节快照见 `admin_snapshot.rs`；这里按接口写成可读的用例（鉴权、校验文案、返回结构、写进库的行、操作日志、导出内容）。

mod common;
use common::{query, ready, send};
use serde_json::{Value, json};
use std::io::Read;

const A: &str = "/admin/api";

/// 8×8 的合成 PNG。
const PNG_HEX: &str = "89504e470d0a1a0a0000000d49484452000000080000000808020000004b6d29dc0000006c4944415478da0dc94101003008033194a0a44aaa84e7a940094aaa68cb375545172a5c4cb1c51529aa9a6ed4b899669b6bd23f440b098b112b4e443f4c1b199b316bcec43f861e34789861871b323f965eb4789965975bb23f8e3e74f898638f3b723f42070587091b2e243cb02c5481f9a60b790000000049454e44ae426082";

fn zip_files(blob: &[u8]) -> Vec<(String, Vec<u8>)> {
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(blob)).expect("是 zip");
    (0..z.len())
        .map(|i| {
            let mut f = z.by_index(i).unwrap();
            let mut data = Vec::new();
            f.read_to_end(&mut data).unwrap();
            (f.name().unwrap().to_string(), data)
        })
        .collect()
}

#[tokio::test]
async fn audit_path_ids_are_checked_after_login() {
    let (e, a) = ready().await;
    // 先鉴权再校验参数；写请求的校验失败记成失败的操作
    send(
        &e.state,
        "GET",
        &format!("{A}/audit/x/thumb"),
        None,
        None,
        &[],
    )
    .await
    .assert_detail(401, "未登录或会话已过期");
    a.get(&format!("{A}/audit/x/thumb"))
        .await
        .assert_detail(422, "audit_id 必须是整数");
    // 登录会记录一条；GET 校验失败不写操作日志。
    let acts = a.actions().await;
    assert_eq!(acts.len(), 1, "{acts:?}");
}

#[tokio::test]
async fn usage_logs_filter_and_paginate() {
    let (e, a) = ready().await;
    // 两把 Key 的日志 + 各种功能
    a.sql("INSERT INTO api_keys(id, name, token, created_at) VALUES (2, '二号', 'nai-t-2', 1.0);")
        .await;
    let mut sql = String::new();
    for i in 0..45 {
        let (kid, kind) = match i % 3 {
            0 => (1, "image"),
            1 => (2, "upscale"),
            _ => (1, "chat"),
        };
        sql.push_str(&format!(
            "INSERT INTO usage_log(ts, key_id, key_name, kind, model, status, images, rid) VALUES ({}.0, {kid}, 'k{kid}', '{kind}', 'm', 'ok', 1, 'ab{:02x}');",
            1000 + i,
            i
        ));
    }
    a.sql(&sql).await;
    let r = a.get(&format!("{A}/logs")).await;
    assert_eq!(r.status, 200, "{}", r.text());
    let v = r.json();
    assert_eq!(
        (
            v["page"].clone(),
            v["per_page"].clone(),
            v["total"].clone(),
            v["pages"].clone()
        ),
        (json!(1), json!(20), json!(45), json!(3))
    );
    assert_eq!(v["logs"].as_array().unwrap().len(), 20);
    assert_eq!(v["logs"][0]["id"], 45, "新的在前");
    let cols: Vec<&str> = v["logs"][0]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        &cols[..6],
        ["id", "ts", "key_id", "key_name", "kind", "model"]
    );
    // 原因短说明和原因下拉选项由服务端给
    assert!(v["logs"][0]["reason_label"].is_string());
    let reasons = v["reasons"].as_array().unwrap();
    assert_eq!(reasons.len(), gate_core::reasons::LABELS.len());
    assert_eq!(
        reasons[0],
        json!({"code": gate_core::reasons::LABELS[0].0, "label": gate_core::reasons::LABELS[0].1})
    );
    // 页码夹取：越界回最后一页（5 条），0 / 负数回第一页
    assert_eq!(
        a.get(&format!("{A}/logs?page=99")).await.json()["logs"]
            .as_array()
            .unwrap()
            .len(),
        5
    );
    assert_eq!(a.get(&format!("{A}/logs?page=-3")).await.json()["page"], 1);
    // 每页条数只认 20 / 50 / 100
    assert_eq!(
        a.get(&format!("{A}/logs?per_page=50")).await.json()["pages"],
        1
    );
    assert_eq!(
        a.get(&format!("{A}/logs?per_page=7")).await.json()["per_page"],
        20
    );
    // 筛选
    assert_eq!(
        a.get(&format!("{A}/logs?key_id=2")).await.json()["total"],
        15
    );
    assert_eq!(
        a.get(&format!("{A}/logs?key_id=0")).await.json()["total"],
        45,
        "0 = 不筛"
    );
    assert_eq!(
        a.get(&format!("{A}/logs?feature=upscale")).await.json()["total"],
        15
    );
    // 文本生成 / 语音已删：不能再按它们筛选；旧的 chat / text 记录仍在全部列表里
    for f in ["text", "voice"] {
        a.get(&format!("{A}/logs?feature={f}"))
            .await
            .assert_detail(422, "未知功能");
    }
    assert_eq!(
        a.get(&format!("{A}/logs?feature=")).await.json()["total"],
        45
    );
    assert_eq!(
        a.get(&format!("{A}/logs?rid=%20AB0F%2Czz%20")).await.json()["total"],
        1,
        "rid 只留十六进制字符"
    );
    assert_eq!(
        a.get(&format!("{A}/logs?rid=zz")).await.json()["total"],
        45,
        "过滤后为空 = 不筛"
    );
    assert_eq!(
        v["summary"],
        json!({"total": 45, "ok": 45, "rejected": 0, "error": 0})
    );
    // 状态 / 来源 IP / 原因码 / 多把 Key；行里带 UA（client）
    a.sql(
        "INSERT INTO usage_log(ts, key_id, key_name, kind, model, status, reason, src, client, token_id) VALUES \
         (2000.0, 1, 'k1', 'image', 'm', 'rejected', 'queue_full', '203.0.113.9', 'axios/1.7', NULL), \
         (2001.0, 2, 'k2', 'image', 'm', 'rejected', 'key_rate', '203.0.113.9', 'curl/8', NULL), \
         (2002.0, 3, 'k3', 'image', 'm', 'error', '', '198.51.100.4', 'Mozilla/5.0', 'tok-a');",
    )
    .await;
    let v = a.get(&format!("{A}/logs?status=rejected")).await.json();
    assert_eq!(v["total"], 2);
    assert_eq!(
        v["summary"],
        json!({"total": 48, "ok": 45, "rejected": 2, "error": 1}),
        "summary 不受 status 自己影响"
    );
    assert_eq!(v["logs"][0]["client"], "curl/8");
    assert_eq!(v["logs"][0]["src"], "203.0.113.9");
    let v = a
        .get(&format!("{A}/logs?ip=%20203.0.113.9%20"))
        .await
        .json();
    assert_eq!(v["total"], 2, "同一个 IP 用过的所有 Key");
    assert_eq!(
        v["summary"],
        json!({"total": 2, "ok": 0, "rejected": 2, "error": 0})
    );
    assert_eq!(
        a.get(&format!("{A}/logs?reason=queue_full")).await.json()["total"],
        1
    );
    let v = a.get(&format!("{A}/logs?status=error")).await.json();
    assert_eq!(
        (v["total"].clone(), v["logs"][0]["token_id"].clone()),
        (json!(1), json!("tok-a"))
    );
    assert_eq!(
        a.get(&format!("{A}/logs?key_id=1,3&status=rejected"))
            .await
            .json()["total"],
        1
    );
    assert_eq!(
        a.get(&format!("{A}/logs?key_id=1,2")).await.json()["total"],
        47
    );
    for (q, msg) in [
        ("status=oops", "未知状态"),
        ("ip=203.0.113", "IP 格式不对"),
        ("ip=203.0.113.9;drop", "IP 格式不对"),
        ("reason=Queue-Full", "未知原因码"),
        ("key_id=1,x", "Key 编号无效"),
        ("key_id=1,", "Key 编号无效"),
    ] {
        a.get(&format!("{A}/logs?{q}"))
            .await
            .assert_detail(422, msg);
    }
    // 校验
    a.get(&format!("{A}/logs?feature=nope"))
        .await
        .assert_detail(422, "未知功能");
    a.get(&format!("{A}/logs?key_id=a&page=b"))
        .await
        .assert_detail(422, "key_id 必须是整数");
    // 只认规范写法：小数、下划线都不接受
    for q in ["page=1.0", "page=1_0"] {
        assert_eq!(a.get(&format!("{A}/logs?{q}")).await.status, 422, "{q}");
    }
    drop(e);
}

#[tokio::test]
async fn admin_actions_filters() {
    let (_e, a) = ready().await;
    a.sql(
        "INSERT INTO admin_actions(ts, actor, action, target, detail, ok) VALUES \
         (1.0, '站长', '添加黑名单', '', '', 1), (2.0, '系统', '私信成员', '', '', 1), \
         (3.0, 'Discord:900000000000000001', '成员自助重置', '', '', 1), (4.0, '后台 *.*.0.1', '重置今日额度', '', '', 0);",
    )
    .await;
    let all = a.get(&format!("{A}/actions")).await.json();
    assert_eq!(
        (
            all["per_page"].clone(),
            all["total"].clone(),
            all["pages"].clone()
        ),
        (json!(30), json!(5), json!(1))
    );
    assert_eq!(
        all["action_kinds"],
        json!([
            "成员自助重置",
            "添加黑名单",
            "登录后台",
            "私信成员",
            "重置今日额度"
        ]),
        "按字节序去重"
    );
    let names = |v: Value| -> Vec<String> {
        v["actions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["action"].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(
        names(a.get(&format!("{A}/actions?actor=admin")).await.json()),
        ["重置今日额度", "添加黑名单", "登录后台"]
    );
    assert_eq!(
        names(a.get(&format!("{A}/actions?actor=system")).await.json()),
        ["私信成员"]
    );
    assert_eq!(
        names(a.get(&format!("{A}/actions?actor=member")).await.json()),
        ["成员自助重置"]
    );
    assert_eq!(
        names(a.get(&format!("{A}/actions?actor=zzz")).await.json()).len(),
        5,
        "未知类别不筛"
    );
    let q = "action=%E6%B7%BB%E5%8A%A0%E9%BB%91%E5%90%8D%E5%8D%95%7C%20%E7%A7%81%E4%BF%A1%E6%88%90%E5%91%98%20%7C%7C";
    assert_eq!(
        names(a.get(&format!("{A}/actions?{q}")).await.json()),
        ["私信成员", "添加黑名单"],
        "| 分隔多个操作名"
    );
    assert_eq!(
        a.get(&format!("{A}/actions?page=50")).await.json()["page"],
        1
    );
    let first = &all["actions"][0];
    assert_eq!(first["action"], "重置今日额度");
    a.get(&format!("{A}/actions?page=q"))
        .await
        .assert_detail(422, "page 必须是整数");
}

#[tokio::test]
async fn bug_tracker_lists_resolves_and_names_keys() {
    let (e, a) = ready().await;
    a.sql(
        "INSERT INTO api_keys(id, name, token, created_at) VALUES (2, '', 'nai-t-2', 1.0);\
         INSERT INTO error_events(sig, source, level, title, detail, count, first_seen, last_seen, last_rid, last_key, last_path, resolved_at) VALUES \
         ('aaaaaaaaaaaa', 'request', 'error', 'A', 'd', 3, 1.0, 50.0, 'r1', 1, '/x', NULL), \
         ('bbbbbbbbbbbb', 'upstream', 'error', 'B', 'd', 1, 1.0, 40.0, '', 99, '', NULL), \
         ('cccccccccccc', 'web:landing', 'warn', 'C', 'd', 1, 1.0, 30.0, '', NULL, '', NULL), \
         ('dddddddddddd', 'request', 'error', 'D', 'd', 2, 1.0, 20.0, '', 2, '', 5.0);",
    )
    .await;
    let v = a.get(&format!("{A}/errors")).await.json();
    assert_eq!(v["open"], 3);
    let names: Vec<(&str, &str)> = v["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["title"].as_str().unwrap(),
                r["key_name"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        names,
        [("A", "测试成员"), ("B", "Key #99 (已删除)"), ("C", "")],
        "已处理的默认不列；没有 Key 为空串"
    );
    let all = a.get(&format!("{A}/errors?all=true")).await.json();
    let last = &all["errors"][3];
    assert_eq!(last["title"], "D");
    assert_eq!(
        last["key_name"], "Key #2 (已删除)",
        "名字是空串也当作已删除（旧版如此）"
    );
    let order: Vec<&str> = last
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        order,
        [
            "sig",
            "source",
            "level",
            "title",
            "detail",
            "count",
            "first_seen",
            "last_seen",
            "last_rid",
            "last_key",
            "last_path",
            "resolved_at",
            "auto_resolved", // 3.0 新增
            "key_name"
        ]
    );
    a.post(&format!("{A}/errors/nonexistent/resolve"), json!({}))
        .await
        .assert_detail(404, "没有这条错误记录");
    // sig 只取前 12 个字符；重复标记也成功
    assert_eq!(
        a.post(&format!("{A}/errors/aaaaaaaaaaaaZZZZ/resolve"), json!({}))
            .await
            .json(),
        json!({"ok": true})
    );
    assert_eq!(
        a.post(&format!("{A}/errors/aaaaaaaaaaaa/resolve"), json!({}))
            .await
            .json(),
        json!({"ok": true})
    );
    assert_eq!(a.get(&format!("{A}/errors")).await.json()["open"], 2);
    assert_eq!(
        a.post(&format!("{A}/errors/resolve-all"), json!({}))
            .await
            .json(),
        json!({"ok": true, "count": 2})
    );
    assert_eq!(
        a.post(&format!("{A}/errors/resolve-all"), json!({}))
            .await
            .json(),
        json!({"ok": true, "count": 0})
    );
    assert_eq!(
        query(
            &e.state.db,
            "SELECT COUNT(*) AS n FROM error_events WHERE resolved_at IS NULL"
        )
        .await,
        [json!({"n": 0})]
    );
    let acts = a.actions().await;
    assert!(
        acts.iter()
            .any(|(act, t, ..)| act == "标记 Bug 已处理" && t == "Bug aaaaaaaaaaaa"),
        "{acts:?}"
    );
    assert!(
        acts.iter()
            .any(|(act, t, ..)| act == "全部标记 Bug 已处理" && t == "全部未处理 Bug")
    );
}

fn audit_seed(png: &str) -> String {
    format!(
        "INSERT INTO api_keys(id, name, token, created_at) VALUES (2, '白 鸟/x', 'nai-t-2', 1.0);\
         INSERT INTO generation_audit(ts, key_id, key_name, prompt, negative, extra, image, image_type, image_size) VALUES \
         (86400.0, 1, '测试成员', 'cat girl', 'lowres', '{{\"params\":{{\"width\":832,\"height\":1216,\"seed\":123,\"sampler\":\"k_euler\",\"steps\":28,\"scale\":5.0}}}}', X'{png}', 'image/png', 165), \
         (90000.0, 1, '测试成员', 'owl', '', '{{\"params\":{{\"seed\":0,\"sampler\":\"\",\"steps\":0,\"scale\":0}}}}', X'{png}', '', 165), \
         (91000.0, 1, '测试成员', '只有缩略图', '', '', NULL, '', NULL), \
         (92000.0, 2, '白 鸟/x', 'moon', 'bad', 'not json', X'{png}', 'image/webp', 165), \
         (93000.0, 2, '白 鸟/x', '没有图', '', '', NULL, '', NULL);\
         UPDATE generation_audit SET thumb = X'ffd8ffe066616b65' WHERE id = 3;"
    )
}

#[tokio::test]
async fn audit_gallery_images_export_and_purge() {
    let (e, a) = ready().await;
    a.sql(&audit_seed(PNG_HEX)).await;
    let v = a.get(&format!("{A}/audit")).await.json();
    assert_eq!(
        (
            v["total"].clone(),
            v["per_page"].clone(),
            v["pages"].clone()
        ),
        (json!(5), json!(24), json!(1))
    );
    let first = &v["items"][0];
    let order: Vec<&str> = first
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        order,
        [
            "id",
            "ts",
            "rid",
            "key_id",
            "key_name",
            "kind",
            "model",
            "status",
            "prompt",
            "negative",
            "extra",
            "has_thumb",
            "has_image"
        ]
    );
    assert_eq!(
        (first["has_thumb"].clone(), first["has_image"].clone()),
        (json!(0), json!(0))
    );
    assert_eq!(v["items"][2]["has_thumb"], 1);
    // key_id：单个 Key，多个用逗号；小数写法（"1.0"）、垃圾、空串都查不到
    assert_eq!(
        a.get(&format!("{A}/audit?key_id=2")).await.json()["total"],
        2
    );
    assert_eq!(
        a.get(&format!("{A}/audit?key_id=1")).await.json()["total"],
        3
    );
    assert_eq!(
        a.get(&format!("{A}/audit?key_id=1.0")).await.json()["total"],
        0
    );
    assert_eq!(
        a.get(&format!("{A}/audit?key_id=1,2")).await.json()["total"],
        5
    );
    assert_eq!(
        a.get(&format!("{A}/audit?key_id=2,x,,9")).await.json()["total"],
        2
    );
    for q in ["abc", "", "a,b"] {
        assert_eq!(
            a.get(&format!("{A}/audit?key_id={q}")).await.json()["total"],
            0,
            "{q}"
        );
    }
    assert_eq!(
        a.get(&format!("{A}/audit?per_page=48")).await.json()["per_page"],
        48
    );
    assert_eq!(
        a.get(&format!("{A}/audit?per_page=5")).await.json()["per_page"],
        24
    );
    assert_eq!(a.get(&format!("{A}/audit?page=9")).await.json()["page"], 1);

    // 缩略图：优先用存好的；没有就从原图现缩（JPEG）；都没有 404
    let r = a.get(&format!("{A}/audit/3/thumb")).await;
    assert_eq!(
        (
            r.status,
            r.headers["content-type"].to_str().unwrap(),
            r.body.clone()
        ),
        (
            200,
            "image/jpeg",
            vec![0xff, 0xd8, 0xff, 0xe0, 0x66, 0x61, 0x6b, 0x65]
        )
    );
    let r = a.get(&format!("{A}/audit/1/thumb")).await;
    assert_eq!(r.status, 200);
    assert_eq!(&r.body[..2], [0xff, 0xd8]);
    a.get(&format!("{A}/audit/5/thumb"))
        .await
        .assert_detail(404, "没有图片（原图已过期或未记录）");
    a.get(&format!("{A}/audit/999/thumb"))
        .await
        .assert_detail(404, "没有图片（原图已过期或未记录）");
    // 原图：保存时的类型；类型为空回落 png；没有原图回落缩略图
    let png = hex_bytes(PNG_HEX);
    let r = a.get(&format!("{A}/audit/1/image")).await;
    assert_eq!(
        (r.headers["content-type"].to_str().unwrap(), r.body.clone()),
        ("image/png", png.clone())
    );
    assert_eq!(
        a.get(&format!("{A}/audit/2/image")).await.headers["content-type"],
        "image/png"
    );
    assert_eq!(
        a.get(&format!("{A}/audit/4/image")).await.headers["content-type"],
        "image/webp"
    );
    let r = a.get(&format!("{A}/audit/3/image")).await;
    assert_eq!(
        (r.headers["content-type"].to_str().unwrap(), r.body.len()),
        ("image/jpeg", 8)
    );
    a.get(&format!("{A}/audit/5/image"))
        .await
        .assert_detail(404, "没有图片");

    // 导出：某成员的全部原图 + prompts.txt
    a.get(&format!("{A}/audit/export"))
        .await
        .assert_detail(400, "缺少 key_id");
    for q in ["3", "999", "abc"] {
        a.get(&format!("{A}/audit/export?key_id={q}"))
            .await
            .assert_detail(404, "该成员没有可导出的原图");
    }
    // 多个 key_id：合并成一个 zip，每个有记录的账号一个文件夹（原图过期也有提示词）；都没记录就 404
    a.get(&format!("{A}/audit/export?key_id=3,999"))
        .await
        .assert_detail(404, "选中的成员没有生成记录");
    let r = a.get(&format!("{A}/audit/export?key_id=1,2")).await;
    assert_eq!(r.status, 200);
    assert_eq!(r.headers["content-type"], "application/zip");
    let names: Vec<String> = zip_files(&r.body).into_iter().map(|(n, _)| n).collect();
    assert!(names.iter().any(|n| n == "说明.txt"));
    assert!(names.iter().any(|n| n.ends_with("/prompts.txt")));
    assert!(names.iter().any(|n| n.contains('/') && !n.starts_with('/')));
    let r = a.get(&format!("{A}/audit/export?key_id=2")).await;
    assert_eq!(r.status, 200);
    assert_eq!(r.headers["content-type"], "application/zip");
    let disp = r.headers["content-disposition"].to_str().unwrap();
    assert_eq!(
        disp,
        "attachment; filename=\"owl-x-1.zip\"; filename*=UTF-8''owl-%E7%99%BD%20%E9%B8%9F/x-1.zip",
        "文件名只留字母数字和 _.-，其余换成 _ 并去掉两头的 _.；filename* 保留原名（/ 不转义）"
    );
    let files = zip_files(&r.body);
    assert_eq!(files.len(), 2);
    assert!(files[0].0.ends_with(".webp") && files[0].0.starts_with("0001_"));
    assert_eq!(files[0].1, png);
    let text = String::from_utf8(files[1].1.clone()).unwrap();
    assert!(
        text.starts_with("白 鸟/x 的作品 · 共 1 张 · 导出于 "),
        "{text}"
    );
    assert!(text.contains("\n正面：moon\n负面：bad\n"), "{text}");
    assert!(!text.contains("参数："), "extra 不是 JSON 就没有参数行");
    let r = a.get(&format!("{A}/audit/export?key_id=1")).await;
    let files = zip_files(&r.body);
    let names: Vec<&str> = files.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names.len(), 3);
    assert!(
        names[0].starts_with("0001_")
            && names[0].ends_with(".png")
            && names[1].starts_with("0002_")
    );
    let text = String::from_utf8(files[2].1.clone()).unwrap();
    assert!(
        text.contains(
            "正面：cat girl\n负面：lowres\n参数：种子 123 · k_euler · 28步 · CFG 5.0 · 832×1216\n"
        ),
        "{text}"
    );
    assert!(
        text.contains("正面：owl\n负面：\n参数：种子 0 · CFG 0\n"),
        "种子 0 也算有，采样器 / 步数为 0 不显示: {text}"
    );

    // 缩略图有进程级缓存：清空原图之前翻页回来不再重算；清空接口会清缓存
    let before = a.get(&format!("{A}/audit/1/thumb")).await.body;
    e.state
        .db
        .purge_audit_images_before_id(1 << 40)
        .await
        .unwrap();
    assert_eq!(
        a.get(&format!("{A}/audit/1/thumb")).await.body,
        before,
        "缓存命中，原图已被后台任务清掉也照旧能看"
    );
    a.post(&format!("{A}/audit/purge-images"), json!({}))
        .await
        .assert_detail(422, "需要 confirm=清空");
    a.post(
        &format!("{A}/audit/purge-images"),
        json!({"confirm": "yes"}),
    )
    .await
    .assert_detail(422, "需要 confirm=清空");
    a.sql(&format!("UPDATE generation_audit SET image = X'{PNG_HEX}', image_type='image/png' WHERE id IN (1, 4)")).await;
    let r = a
        .post(
            &format!("{A}/audit/purge-images"),
            json!({"confirm": "清空"}),
        )
        .await;
    assert_eq!(r.json(), json!({"ok": true, "purged": 2}));
    a.get(&format!("{A}/audit/1/thumb"))
        .await
        .assert_detail(404, "没有图片（原图已过期或未记录）");
    assert_eq!(
        a.get(&format!("{A}/audit")).await.json()["total"],
        5,
        "只清原图，提示词和参数照留"
    );
    let acts = a.actions().await;
    // 成功那次是处理函数自己记的日志，中间件不重复记；两次确认没过各记一条失败
    assert_eq!(
        acts.iter()
            .filter(|(act, _, _, ok)| act == "清空原图" && *ok)
            .count(),
        1,
        "{acts:?}"
    );
    assert_eq!(
        acts.iter()
            .filter(|(act, _, _, ok)| act == "清空原图" && !*ok)
            .count(),
        2,
        "{acts:?}"
    );
    assert!(
        acts.iter()
            .any(|(act, t, d, ok)| act == "清空原图" && t.is_empty() && d == "2 张" && *ok)
    );
}

fn hex_bytes(h: &str) -> Vec<u8> {
    (0..h.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&h[i..i + 2], 16).unwrap())
        .collect()
}

#[tokio::test]
async fn feedback_list_and_csv_export() {
    let (_e, a) = ready().await;
    // /反馈 现在是一段文字；旧版选择题的记录（id 2）照常显示成一行摘要
    a.sql(
        r#"INSERT INTO api_keys(id, name, token, created_at) VALUES (2, '乙', 'nai-t-2', 1.0);
         INSERT INTO feedback(ts, discord_id, username, key_id, answers) VALUES
         (200.0, '900000000000000002', 'plain', 2, '{"text": "排队有点久,\n\"希望\"快一点"}'),
         (100.0, '900000000000000001', 'user1', 1, '{"score": "满意", "pain": "排队太久 / 经常提示人多", "wish": "多给一些 V5", "freq": "每天都用"}'),
         (300.0, '900000000000000099', '=cmd,"x"', 99, '{}'),
         (400.0, '900000000000000004', 'u4', NULL, '{"text": "=HYPERLINK(1)"}');"#,
    )
    .await;
    let v = a.get(&format!("{A}/feedback")).await.json();
    assert!(v.get("questions").is_none(), "题目已经去掉: {v}");
    let items = v["items"].as_array().unwrap();
    assert_eq!(
        items
            .iter()
            .map(|i| i["id"].as_i64().unwrap())
            .collect::<Vec<_>>(),
        [4, 3, 1, 2],
        "最新在前"
    );
    // 只有这几个字段（不再带 answers）
    assert_eq!(
        items[0]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        [
            "id",
            "ts",
            "discord_id",
            "username",
            "key_id",
            "key_name",
            "text",
            "legacy"
        ]
    );
    assert_eq!(items[0]["text"], "=HYPERLINK(1)");
    assert_eq!(items[0]["legacy"], false);
    assert_eq!(items[0]["key_name"], "");
    assert!(items[0]["key_id"].is_null());
    assert_eq!(items[1]["text"], "", "什么都没有的记录文字为空");
    assert_eq!(items[1]["legacy"], false);
    assert_eq!(items[2]["text"], "排队有点久,\n\"希望\"快一点");
    assert_eq!(items[2]["legacy"], false);
    assert_eq!(items[2]["key_name"], "乙");
    // 旧版选择题：缺的题目跳过，其余拼成一行
    assert_eq!(
        items[3]["text"],
        "（旧版选择题）整体：满意；困扰：排队太久 / 经常提示人多；希望：多给一些 V5；频率：每天都用"
    );
    assert_eq!(items[3]["legacy"], true);
    assert_eq!(items[3]["key_name"], "测试成员");

    let r = a.get(&format!("{A}/feedback/export")).await;
    assert_eq!(r.status, 200);
    assert_eq!(r.headers["content-type"], "text/csv; charset=utf-8");
    let disp = r.headers["content-disposition"].to_str().unwrap();
    assert!(
        disp.starts_with("attachment; filename=\"owl-feedback-") && disp.ends_with(".csv"),
        "{disp}"
    );
    assert!(disp.contains("; filename*=UTF-8''owl-feedback-"), "{disp}");
    let text = r.text();
    assert!(text.starts_with('\u{feff}'));
    // 行尾是 \r\n，单元格里的换行只有 \n，所以按 \r\n 切就是按记录切
    let lines: Vec<&str> = text.trim_start_matches('\u{feff}').split("\r\n").collect();
    assert_eq!(lines[0], "time,member,text");
    assert!(
        lines[1].starts_with("1970-01-01 ")
            && lines[1].ends_with(
                ",测试成员,（旧版选择题）整体：满意；困扰：排队太久 / 经常提示人多；希望：多给一些 V5；频率：每天都用"
            ),
        "按时间升序，旧记录同一行摘要: {}",
        lines[1]
    );
    assert!(
        lines[2].ends_with(",乙,\"排队有点久,\n\"\"希望\"\"快一点\""),
        "成员取 Key 名，文字里的逗号 / 换行 / 引号按 CSV 转义: {}",
        lines[2]
    );
    assert!(
        lines[3].ends_with(",\"'=cmd,\"\"x\"\"\","),
        "没有 Key 名用用户名；公式防注入 + CSV 转义；空文字: {}",
        lines[3]
    );
    assert!(
        lines[4].ends_with(",u4,'=HYPERLINK(1)"),
        "文字也防公式注入: {}",
        lines[4]
    );
    assert_eq!(lines.len(), 6, "表头 + 4 条 + 末尾空串");
    assert!(
        a.actions()
            .await
            .iter()
            .any(|(act, t, ..)| act == "导出成员反馈" && t == "全部CSV")
    );
}

#[tokio::test]
async fn survey_results_are_available() {
    let (_e, a) = ready().await;
    a.sql(
        "INSERT INTO api_keys(id, name, token, created_at) VALUES (2, '乙', 'nai-t-2', 1.0);\
         INSERT INTO member_survey(key_id, client, understand, improve, improve_other, donate, ts) VALUES \
         (1, '柏宝绘', '懂了', '多给 V5、出图更快、多给 V5', '', '愿意', 10.0), \
         (2, '手机', '懂了', '出图更快、 、', '希望有公告', '再想想', 20.0);",
    )
    .await;
    let v = a.get(&format!("{A}/survey")).await.json();
    assert_eq!(v["total"], 2);
    assert_eq!(v["rows"][0]["name"], "乙", "新的在前");
    assert_eq!(v["rows"][1]["discord_id"], Value::Null);
    assert!(
        v.get("summary").is_none(),
        "选项汇总由前端按明细算，接口不再返回"
    );

    a.get(&format!("{A}/donations"))
        .await
        .assert_detail(404, "Not Found");
}

#[tokio::test]
async fn reset_all_keys_disables_everyone_but_invites() {
    let (e, a) = ready().await;
    a.sql(
        "INSERT INTO api_keys(id, name, token, created_at) VALUES (2, '普通号2', 'nai-t-2', 1.0);\
         INSERT INTO api_keys(id, name, token, created_at) VALUES (3, '邀请号', 'nai-t-3', 1.0), (4, '普通号', 'nai-t-4', 1.0);\
         INSERT INTO key_tags(key_id, tag, note, by, ts) VALUES (3, '邀请', '', '站长', 1.0), (3, '观察', '', '站长', 1.0), (4, '观察', '', '站长', 1.0), (2, '常驻', '', '站长', 1.0);\
         INSERT INTO discord_registrations(discord_id, key_id, created_at) VALUES ('900000000000000003', 3, 1.0), ('900000000000000004', 4, 1.0), ('900000000000000001', 1, 1.0);\
         INSERT INTO share_state(key_id, paused_until) VALUES (4, 9.0);",
    )
    .await;
    a.post(&format!("{A}/ops/reset-all-keys"), json!({}))
        .await
        .assert_detail(400, "安全确认未通过：必须传入 confirm='确认重置'");
    a.post(
        &format!("{A}/ops/reset-all-keys"),
        json!({"confirm": "确认"}),
    )
    .await
    .assert_detail(400, "安全确认未通过：必须传入 confirm='确认重置'");
    let r = a
        .post(
            &format!("{A}/ops/reset-all-keys"),
            json!({"confirm": "确认重置"}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(
        r.json(),
        json!({"ok": true, "disabled_count": 3, "kept_invites": 1,
               "message": "全站已成功重置！已失效 3 把 Key 并解除 Discord 绑定；1 把「邀请」Key 原样保留。"})
    );
    let enabled = query(&e.state.db, "SELECT id, enabled FROM api_keys ORDER BY id").await;
    assert_eq!(
        enabled,
        [
            json!({"id": 1, "enabled": 0}),
            json!({"id": 2, "enabled": 0}),
            json!({"id": 3, "enabled": 1}),
            json!({"id": 4, "enabled": 0})
        ]
    );
    assert_eq!(
        query(&e.state.db, "SELECT discord_id FROM discord_registrations").await,
        [json!({"discord_id": "900000000000000003"})]
    );
    let tags = query(
        &e.state.db,
        "SELECT key_id, tag FROM key_tags ORDER BY key_id, tag",
    )
    .await;
    assert_eq!(
        tags,
        [
            json!({"key_id": 3, "tag": "观察"}),
            json!({"key_id": 3, "tag": "邀请"})
        ],
        "邀请 Key 的标签原样保留，其它 Key 的标签清掉"
    );
    assert!(
        query(&e.state.db, "SELECT 1 FROM share_state")
            .await
            .is_empty()
    );
    // 成功那次是处理函数自己记的日志（站长 / 全站重置 Key），中间件不重复记；确认没过的两次记失败
    let acts = a.actions().await;
    assert_eq!(
        acts.iter()
            .filter(|(act, _, _, ok)| act == "全站杀头重置" && !*ok)
            .count(),
        2,
        "{acts:?}"
    );
    assert_eq!(
        acts.iter()
            .filter(|(act, _, _, ok)| act == "全站杀头重置" && *ok)
            .count(),
        0,
        "{acts:?}"
    );
    assert!(acts.iter().any(|(act, t, d, ok)| act == "全站重置 Key"
        && t == "全部非邀请 Key"
        && d == "已失效 3 把 Key 并清空 Discord 绑定；「邀请」Key 保留 1 把不受影响"
        && *ok));
}
