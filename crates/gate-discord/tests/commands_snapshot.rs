//! 快照：奶妹发给 Discord 的命令定义必须和 `tests/snapshots/commands.json` 逐字符一致（含键顺序）。
//!
//! 改命令名 / 描述 / 选项时，成员在 Discord 里看到的东西就变了，所以要同时更新快照，评审时看 git diff 即可：
//! `UPDATE_SNAPSHOT=1 cargo test -p gate-discord --test commands_snapshot` 会用当前输出覆盖快照文件。

mod common;

use common::{APP, GUILD, MockServer};
use gate_discord::api::DiscordApi;
use gate_discord::commands::{self, NAIMEI_INTENTS};
use serde_json::Value;

const SNAPSHOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/snapshots/commands.json");

fn current() -> String {
    let mut text = serde_json::to_string_pretty(&Value::Array(commands::definitions())).unwrap();
    text.push('\n');
    text
}

#[test]
fn command_definitions_equal_the_snapshot() {
    if std::env::var_os("UPDATE_SNAPSHOT").is_some() {
        std::fs::create_dir_all(std::path::Path::new(SNAPSHOT).parent().unwrap()).unwrap();
        std::fs::write(SNAPSHOT, current()).unwrap();
    }
    let expected = std::fs::read_to_string(SNAPSHOT).expect("读快照（UPDATE_SNAPSHOT=1 可生成）");
    assert_eq!(
        current(),
        expected,
        "命令定义和快照不一致；确认是有意的改动后用 UPDATE_SNAPSHOT=1 更新快照"
    );
}

#[test]
fn only_the_member_commands_are_registered() {
    let defs = commands::definitions();
    let names: Vec<&str> = defs.iter().map(|d| d["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["register", "反馈", "resetkey"]);
    for d in &defs {
        // 所有人都能用：没有权限限制；只注册斜杠命令，没有右键菜单命令
        assert!(d["default_member_permissions"].is_null(), "{d}");
        assert_eq!(d["type"], 1);
        // 成员看到的描述不提已删除的东西
        let desc = d["description"].as_str().unwrap();
        assert!(
            !desc.contains("服务状态") && !desc.contains("管理员"),
            "{desc}"
        );
    }
}

#[test]
fn intents_are_guilds_only() {
    // 513 → 1：不再有 GUILD_MESSAGES（不读频道消息，@奶妹 不回应），也没有任何特权 Intent
    assert_eq!(NAIMEI_INTENTS.bits(), 1);
}

/// `PUT` 只发给 DISCORD_GUILD_ID 这一个公会，请求体就是上面那份定义；不碰全局命令。
#[tokio::test]
async fn sync_goes_to_the_one_guild_endpoint_only() {
    let server = MockServer::start().await;
    let api = DiscordApi::new("test-token", Some(server.discord_proxy()));
    server.reset(&Value::Null, &Value::Null);
    api.sync_guild_commands(APP, GUILD.parse().unwrap(), &commands::definitions())
        .await
        .unwrap();
    let calls = server.discord_log();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["method"], "PUT");
    assert_eq!(
        calls[0]["path"],
        format!("/applications/{APP}/guilds/{GUILD}/commands")
    );
    assert_eq!(
        calls[0]["json"].to_string(),
        Value::Array(commands::definitions()).to_string()
    );
    // 机器人 Token 走 Authorization，且带 Bot 前缀
    assert_eq!(calls[0]["authorization"], "Bot test-token");
}
