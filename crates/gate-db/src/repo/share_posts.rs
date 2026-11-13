//! 存图频道的分享记录。同一条 Discord 消息只记一次。

use crate::{Db, Result};
use rusqlite::params;

impl Db {
    /// 记下一次分享。消息 id 已存在则什么都不改（补扫和实时事件会撞车）。
    pub async fn record_share(
        &self,
        discord_id: &str,
        message_id: &str,
        channel_id: &str,
        at: f64,
    ) -> Result<()> {
        let (discord_id, message_id, channel_id) = (
            discord_id.to_string(),
            message_id.to_string(),
            channel_id.to_string(),
        );
        self.write(move |tx| {
            tx.execute(
                "INSERT INTO share_posts(message_id, discord_id, channel_id, created_at) \
                 VALUES(?,?,?,?) ON CONFLICT(message_id) DO NOTHING",
                params![message_id, discord_id, channel_id, at],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn latest_share(&self, discord_id: &str) -> Result<Option<serde_json::Value>> {
        use rusqlite::OptionalExtension;
        let discord_id = discord_id.to_owned();
        self.read(move |c| c.query_row(
            "SELECT message_id,channel_id,created_at FROM share_posts WHERE discord_id=? ORDER BY created_at DESC LIMIT 1",
            [discord_id], |r| Ok(serde_json::json!({"message_id":r.get::<_,String>(0)?,"channel_id":r.get::<_,String>(1)?,"created_at":r.get::<_,f64>(2)?}))).optional()).await
    }

    pub async fn selected_share(
        &self,
        discord_id: &str,
        message_id: &str,
    ) -> Result<Option<serde_json::Value>> {
        use rusqlite::OptionalExtension;
        let (who, id) = (discord_id.to_owned(), message_id.to_owned());
        self.read(move |c|c.query_row("SELECT message_id,channel_id,created_at FROM share_posts WHERE discord_id=? AND message_id=?",params![who,id],|r|Ok(serde_json::json!({"message_id":r.get::<_,String>(0)?,"channel_id":r.get::<_,String>(1)?,"created_at":r.get::<_,f64>(2)?}))).optional()).await
    }

    /// Batch view for the administrator's member list.
    pub async fn sharing_members_since(
        &self,
        since: f64,
    ) -> Result<std::collections::HashSet<String>> {
        self.read(move |c| {
            let mut stmt =
                c.prepare("SELECT DISTINCT discord_id FROM share_posts WHERE created_at>=?")?;
            stmt.query_map([since], |r| r.get(0))?.collect()
        })
        .await
    }

    /// `since`（含）之后这个成员有没有分享记录。
    pub async fn has_share_since(&self, discord_id: &str, since: f64) -> Result<bool> {
        let discord_id = discord_id.to_string();
        self.read(move |c| {
            let found: i64 = c.query_row(
                "SELECT EXISTS(SELECT 1 FROM share_posts WHERE discord_id=? AND created_at>=?)",
                params![discord_id, since],
                |r| r.get(0),
            )?;
            Ok(found != 0)
        })
        .await
    }
}
