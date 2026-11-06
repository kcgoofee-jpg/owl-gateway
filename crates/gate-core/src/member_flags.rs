//! `discord_registrations.member_flags` 的位定义。
//!
//! 只剩这一位：成员退群后又重新加入。写入口在注册服务，读入口是成员规则与后台展示。
//! 原来的反滥用打分引擎（关联分析、指纹撞车、封禁链接识别）已经整体删除。

/// 退群后又重新加入（1 = 是）。
pub const MEMBER_FLAG_DID_REJOIN: i64 = 1;
