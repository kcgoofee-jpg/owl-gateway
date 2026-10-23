//! 成员反馈：`/反馈` 命令弹出 Discord 表单（一段 2–1000 字的文字），文字存库，后台查看与导出。
//!
//! 不私信、不群发：成员自己发起，回执只有本人可见（680009 申诉期规则）。
//!
//! 旧版是 5 道必填单选题；选择题归「问卷」，3.0 起 `/反馈` 只收文字。
//! 库里已有的旧记录（`answers` 是 score / pain / wish / client / freq 的选择）照常显示成一行摘要。

use gate_core::pyc::{strip, take_chars};
use gate_db::{Db, DbError};
use serde_json::{Value, json};

/// 同一个人 10 分钟内只收一份，防刷。
pub const COOLDOWN: f64 = 600.0;
/// 最少 / 最多多少字（去掉首尾空白之后按字符数算；多出来的部分截掉）。
pub const MIN_CHARS: usize = 2;
pub const MAX_CHARS: usize = 1000;

pub const RECEIPT: &str = "收到啦，谢谢你的反馈～奶妹会转给站长 ✨";
pub const TOO_SHORT: &str = "写点什么再提交吧～";
pub const TOO_FREQUENT: &str = "刚刚已经收到你的反馈啦，过 10 分钟再提交新的吧。";

/// 提交失败：旧版抛 `ValueError`（中文原因，机器人原样转述给成员）。
#[derive(Debug)]
pub enum SubmitError {
    Rejected(String),
    Db(DbError),
}

impl From<DbError> for SubmitError {
    fn from(e: DbError) -> Self {
        SubmitError::Db(e)
    }
}

/// 存一份反馈（`feedback.answers` 里存 `{"text": "..."}`）。返回给本人看的回执文字；
/// 太短 / 太频繁返回 [`SubmitError::Rejected`]。
pub async fn submit(
    db: &Db,
    discord_id: &str,
    username: &str,
    key_id: Option<i64>,
    text: &str,
    now: f64,
) -> Result<String, SubmitError> {
    let text = strip(text);
    if text.chars().count() < MIN_CHARS {
        return Err(SubmitError::Rejected(TOO_SHORT.into()));
    }
    let text = take_chars(text, MAX_CHARS);
    if let Some(last) = db.last_feedback_ts(discord_id).await?
        && last != 0.0
        && now - last < COOLDOWN
    {
        return Err(SubmitError::Rejected(TOO_FREQUENT.into()));
    }
    db.insert_feedback(
        now,
        discord_id,
        take_chars(username, 80),
        key_id,
        &gate_core::pyjson::dumps(&json!({"text": text})),
    )
    .await?;
    Ok(RECEIPT.into())
}

/// 旧版选择题的题目 id → 摘要里的简称（顺序即旧版题目顺序）。
const LEGACY_FIELDS: [(&str, &str); 5] = [
    ("score", "整体"),
    ("pain", "困扰"),
    ("wish", "希望"),
    ("client", "客户端"),
    ("freq", "频率"),
];

/// 一份反馈给后台看的文字，以及是不是旧版选择题的记录。
/// 新记录就是成员写的话；旧记录拼成一行「（旧版选择题）整体：满意；困扰：…；希望：…」；什么都没有就是空串。
pub fn display_text(answers: &Value) -> (String, bool) {
    if let Some(text) = answers.get("text").and_then(Value::as_str) {
        return (strip(text).to_string(), false);
    }
    let parts: Vec<String> = LEGACY_FIELDS
        .iter()
        .filter_map(|(id, label)| {
            let v = strip(answers.get(*id)?.as_str()?);
            (!v.is_empty()).then(|| format!("{label}：{v}"))
        })
        .collect();
    if parts.is_empty() {
        (String::new(), false)
    } else {
        (format!("（旧版选择题）{}", parts.join("；")), true)
    }
}

/// 后台看的「成员」：Key 名 → 用户名 → Discord ID，取第一个非空的。
pub fn member_label(item: &Value) -> String {
    ["key_name", "username", "discord_id"]
        .iter()
        .find_map(|k| {
            item.get(*k)
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
        })
        .unwrap_or("")
        .to_string()
}

/// 后台查看用（旧版 `listing`）：新 → 旧。每条带 `text`（显示用的文字）和 `legacy`（是不是旧版选择题）。
pub async fn listing(db: &Db, limit: i64) -> Result<Vec<Value>, DbError> {
    let rows = db.list_feedback(limit).await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            let answers = r
                .get("answers")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .and_then(gate_core::pyjson::loads)
                .unwrap_or_else(|| json!({}));
            let (text, legacy) = display_text(&answers);
            json!({
                "id": r["id"], "ts": r["ts"], "discord_id": r["discord_id"], "username": r["username"],
                "key_id": r["key_id"], "key_name": r.get("name").and_then(Value::as_str).unwrap_or(""),
                "text": text, "legacy": legacy,
            })
        })
        .collect())
}

/// 防 CSV 公式注入：以 = + - @ 制表符 回车开头的文本前加 '（`client_export.safe_cell`）。
pub fn safe_cell(v: &str) -> String {
    if v.starts_with(['=', '+', '-', '@', '\t', '\r']) {
        format!("'{v}")
    } else {
        v.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_rows_show_the_text_as_written() {
        assert_eq!(
            display_text(&json!({"text": "排队有点久"})),
            ("排队有点久".to_string(), false)
        );
    }

    #[test]
    fn old_choice_rows_become_one_readable_line() {
        let old = json!({"score": "满意", "pain": "排队太久 / 经常提示人多", "wish": "多给一些 V5",
                         "client": "手机 App", "freq": "每天都用"});
        assert_eq!(
            display_text(&old),
            (
                "（旧版选择题）整体：满意；困扰：排队太久 / 经常提示人多；希望：多给一些 V5；客户端：手机 App；频率：每天都用"
                    .to_string(),
                true
            )
        );
        // 缺的题目跳过，不认识的键丢掉
        assert_eq!(
            display_text(&json!({"score": " 一般 ", "pain": "", "bogus": "x", "freq": "偶尔用"})),
            ("（旧版选择题）整体：一般；频率：偶尔用".to_string(), true)
        );
        assert_eq!(display_text(&json!({})), (String::new(), false));
    }

    #[test]
    fn member_prefers_key_name_then_username_then_id() {
        let m = |k: &str, u: &str, d: &str| {
            member_label(&json!({"key_name": k, "username": u, "discord_id": d}))
        };
        assert_eq!(m("月见草", "alice", "900000000000000001"), "月见草");
        assert_eq!(m("", "alice", "900000000000000001"), "alice");
        assert_eq!(m("", "", "900000000000000001"), "900000000000000001");
    }

    #[test]
    fn csv_formula_guard() {
        assert_eq!(safe_cell("=1+1"), "'=1+1");
        assert_eq!(safe_cell("正常"), "正常");
    }
}
