//! 奶妹的斜杠命令定义：直接拼 JSON 发给 `PUT /applications/{app}/guilds/{guild}/commands`。
//!
//! 为什么手写 JSON 而不用 twilight 的 `Command` 结构：
//! - 要让 Discord 收到的就是下面这份字面内容：`"contexts": null`、`"integration_types": null`、
//!   `"dm_permission": true` 都显式带上；typed 结构会把 `None` 省略，`PUT` 整体覆盖时
//!   「省略」与「null」对 Discord 是否等价没有文档保证，而封号（680009）之后我们不想靠猜；
//! - 没写描述的参数用占位描述 `…`（U+2026）；
//! - 快照测试（`tests/commands_snapshot.rs`）把当前输出逐字符存在 `tests/snapshots/commands.json`，
//!   改命令名 / 描述 / 选项时要同时改快照，评审时一眼能看到成员在 Discord 里看到的变化。
//!
//! 命令仅注册到指定服务器。确定性只读查询与模型工具共用能力注册表；
//! 私密响应来自斜杠命令或消息菜单，普通消息不能自动收到 ephemeral 回复。

use serde_json::{Value, json};
use twilight_model::gateway::Intents;

/// 奶妹的网关 Intents：GUILDS（服务器名）+ GUILD_MESSAGES + MESSAGE_CONTENT（@奶妹 / 回复奶妹触发对话）。
/// MESSAGE_CONTENT 是特权 Intent，须在开发者后台勾选，否则网关以 4014 拒连（启动即失败退避重试）。
/// 用于对话触发和分享图片检测；论坛记录分享元数据，可在提交后添加确认反应，不私信，其余纪律见 `lib.rs`。
pub const NAIMEI_INTENTS: Intents = Intents::GUILDS
    .union(Intents::GUILD_MESSAGES)
    .union(Intents::MESSAGE_CONTENT);

/// 奶妹的全部斜杠命令（按注册顺序）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Register,
    ResetKey,
}

impl Command {
    pub const ALL: [Command; 2] = [Command::Register, Command::ResetKey];

    pub const fn name(self) -> &'static str {
        match self {
            Command::Register => "register",
            Command::ResetKey => "resetkey",
        }
    }

    pub fn from_name(name: &str) -> Option<Command> {
        Command::ALL.into_iter().find(|c| c.name() == name)
    }
}

/// 一条命令的外壳（所有人可用，没有权限限制）。
fn command(name: &str, description: &str, options: Vec<Value>) -> Value {
    json!({
        "name": name,
        "description": description,
        "type": 1,
        "options": options,
        "nsfw": false,
        "dm_permission": true,
        "default_member_permissions": null,
        "contexts": null,
        "integration_types": null,
    })
}

/// 发给 `PUT /applications/{app}/guilds/{guild}/commands` 的整个数组。
pub fn definitions() -> Vec<Value> {
    let mut defs = vec![
        command(
            Command::Register.name(),
            "领取你的猫头鹰公益站 API Key",
            vec![],
        ),
        command(
            Command::ResetKey.name(),
            "Key 丢了或泄露了？换一把新的（旧的立即失效）",
            vec![],
        ),
    ];
    for c in crate::naimei::capabilities::CAPABILITIES {
        let options = if matches!(c.name, "activity" | "creator_audit") {
            vec![
                json!({"name":"成员","description":if c.name=="creator_audit"{"待查作者；帖子内省略为楼主，其他位置省略查本人"}else{"待查询成员；省略查本人"},"type":6,"required":false}),
            ]
        } else if c.name == "report_client" {
            vec![
                json!({"name":"客户端","description":"你用的生图客户端名称，如 柏宝绘","type":3,"required":true,"max_length":40}),
            ]
        } else {
            vec![]
        };
        defs.push(command(c.command, c.description, options));
    }
    defs.push(command("取消", "取消本人正在等待或处理的聊天", vec![]));
    defs.push(json!({"name":"查看分享状态","type":3,"dm_permission":false}));
    defs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_roundtrip_and_follow_definition_order() {
        let defs = definitions();
        assert_eq!(
            defs.len(),
            Command::ALL.len() + crate::naimei::capabilities::CAPABILITIES.len() + 2
        );
        for (cmd, def) in Command::ALL.iter().zip(&defs) {
            assert_eq!(def["name"], cmd.name());
            assert_eq!(Command::from_name(cmd.name()), Some(*cmd));
        }
        assert_eq!(Command::from_name("nope"), None);
        // 删掉的命令不再认
        for gone in [
            "open", "limit", "ban", "unban", "slots", "revoke", "quota", "help",
        ] {
            assert_eq!(Command::from_name(gone), None, "{gone}");
        }
    }
}
