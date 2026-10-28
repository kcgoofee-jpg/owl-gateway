//! Idempotent schema initialization and migrations for existing gateway databases.
//! Prompt image removal is handled by archive_removal after this schema is initialized.

use rusqlite::{Connection, ErrorCode};

const SCHEMA: &str = include_str!("sql/schema.sql");
const STATUS_SCHEMA: &str = include_str!("sql/status_schema.sql");

/// 老库补列（新库建表已含这些列，执行会报 duplicate column，忽略即可）。顺序决定列序，不要打乱。
const ADD_COLUMNS: &[&str] = &[
    "ALTER TABLE counters ADD COLUMN v5 INTEGER NOT NULL DEFAULT 0",
    "ALTER TABLE api_keys ADD COLUMN features TEXT",
    "ALTER TABLE discord_registrations ADD COLUMN role_granted INTEGER NOT NULL DEFAULT 0",
    "ALTER TABLE discord_registrations ADD COLUMN username TEXT",
    "ALTER TABLE discord_registrations ADD COLUMN display_name TEXT",
    "ALTER TABLE discord_registrations ADD COLUMN avatar TEXT",
    "ALTER TABLE upstream_token_counters ADD COLUMN images INTEGER NOT NULL DEFAULT 0",
    "ALTER TABLE usage_log ADD COLUMN wait_ms INTEGER NOT NULL DEFAULT 0",
    "ALTER TABLE usage_log ADD COLUMN dur_ms INTEGER NOT NULL DEFAULT 0",
    "ALTER TABLE usage_log ADD COLUMN client TEXT NOT NULL DEFAULT ''",
    "ALTER TABLE usage_log ADD COLUMN up_status INTEGER NOT NULL DEFAULT 0",
    "ALTER TABLE usage_log ADD COLUMN rid TEXT NOT NULL DEFAULT ''",
    "ALTER TABLE usage_log ADD COLUMN src TEXT NOT NULL DEFAULT ''",
    "ALTER TABLE usage_log ADD COLUMN ver TEXT NOT NULL DEFAULT ''",
    "ALTER TABLE usage_log ADD COLUMN reason TEXT NOT NULL DEFAULT ''",
    "ALTER TABLE upstream_snapshots ADD COLUMN fresh INTEGER NOT NULL DEFAULT 0",
    "ALTER TABLE discord_registrations ADD COLUMN member_checked_at REAL NOT NULL DEFAULT 0",
    "ALTER TABLE upstream_token_counters ADD COLUMN units REAL",
    "ALTER TABLE share_state ADD COLUMN pause_reason TEXT NOT NULL DEFAULT ''",
    "ALTER TABLE discord_bans ADD COLUMN reason TEXT NOT NULL DEFAULT ''",
    "ALTER TABLE discord_bans ADD COLUMN username TEXT NOT NULL DEFAULT ''",
    "ALTER TABLE discord_bans ADD COLUMN display_name TEXT NOT NULL DEFAULT ''",
    "ALTER TABLE discord_bans ADD COLUMN avatar TEXT NOT NULL DEFAULT ''",
    "ALTER TABLE discord_bans ADD COLUMN banned_by TEXT NOT NULL DEFAULT '系统'",
    "ALTER TABLE api_keys ADD COLUMN last_image_at REAL",
];

/// 3.0 新增的列（nullable，表结构只增不减）。
/// 只放在这里、不写进 `schema.sql`：全新库与线上库走同一条路（建出 2.x 的表再补列），
/// `tests/schema_parity.rs` 的「基线表」一侧也是按这份清单补出来的。
pub const RUST_ADDED_COLUMNS: &[&str] = &[
    // 成员进服时间（Discord 返回的 joined_at，Unix 秒；没取到为 NULL）和成员标记位（1 = 退群后又重新加入）：
    // 防小号（「刚进服就领 Key」「退了又进」）和新号额度爬坡用。
    "ALTER TABLE discord_registrations ADD COLUMN guild_joined_at REAL",
    "ALTER TABLE discord_registrations ADD COLUMN member_flags INTEGER",
    // Key 的停用原因（gate_core::key_status）；启用中 / 2.x 停用的为 NULL。
    "ALTER TABLE api_keys ADD COLUMN disabled_reason TEXT",
    // 这条请求最后由哪个上游账号（token_id）处理；没发到上游（鉴权 / 准入阶段就被拒）和 3.0 之前的老行为 NULL。
    // 不建索引：按账号统计总是先按 ts 范围缩小（idx_log_ts），范围内再按它过滤。
    "ALTER TABLE usage_log ADD COLUMN token_id TEXT",
    // 重置今日额度时 V4.5 免费图（counters.legacy_free_images）的基线，和 anlas / v5 一起记。
    "ALTER TABLE daily_quota_offsets ADD COLUMN legacy INTEGER NOT NULL DEFAULT 0",
    // 重置券 / 重置今日额度要重置哪几项：逗号分隔的 v45 / v5 / anlas（v45 一定在）。
    // 站长 10-18：默认只重置 V4.5；10-18 之前发出的券按当时的说法（三项都重置）保留。
    "ALTER TABLE coupons ADD COLUMN scope TEXT NOT NULL DEFAULT 'v45,v5,anlas'",
    // 请求的客户端特征（站长 10-23，防一人多号 / 共用 Key 举证；见 `repo::usage::ClientHints`）：
    // App 包名（X-Requested-With）、Accept-Language、Sec-CH-UA 三项、请求头指纹、浏览器标记哈希、Origin。
    // 不存 Authorization / Cookie 原文；保留期和 client / src 相同：按后台设置 usage_origin_keep_days（0 = 永久，默认永久），到期由 `purge_usage_origin` 清成 NULL。
    "ALTER TABLE usage_log ADD COLUMN xrw TEXT",
    "ALTER TABLE usage_log ADD COLUMN lang TEXT",
    "ALTER TABLE usage_log ADD COLUMN chua TEXT",
    "ALTER TABLE usage_log ADD COLUMN hfp TEXT",
    "ALTER TABLE usage_log ADD COLUMN bid TEXT",
    "ALTER TABLE usage_log ADD COLUMN origin TEXT",
    // 快照分账号记录（站长 10-24）：每个上游账号各写一行。旧行空串 = 早期写的全站聚合值，
    // 只留作历史，不再参与各账号的恢复速度估算。
    "ALTER TABLE upstream_snapshots ADD COLUMN token_id TEXT NOT NULL DEFAULT ''",
    // Per-key canonical quota modes; appended last so compatibility column order remains stable.
];

/// 3.0 新增的表（只增不减）。同 `RUST_ADDED_COLUMNS`：
pub const RUST_ADDED_TABLES: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS member_rules (\
     discord_id TEXT PRIMARY KEY, overrides TEXT NOT NULL, recycle_not_before REAL)",
    // Durable image request lifecycle. Unknown dispatched outcomes remain held for reconciliation;
    // upstream POSTs are never replayed from this table.
    "CREATE TABLE IF NOT EXISTS request_settlements (\
     request_id TEXT PRIMARY KEY, rid TEXT NOT NULL, key_id INTEGER NOT NULL, status TEXT NOT NULL, \
     day TEXT NOT NULL, anlas REAL NOT NULL DEFAULT 0, v5 INTEGER NOT NULL DEFAULT 0, legacy INTEGER NOT NULL DEFAULT 0, \
     dispatched_at REAL, result_json TEXT, updated_at REAL NOT NULL)",
    // 成员用 Discord OAuth 授权后核验到的身份组（领 Key 的门槛只认这张表，不再用机器人 Token 查）。
    "CREATE TABLE IF NOT EXISTS role_verifications (\
     discord_id TEXT NOT NULL, guild_id TEXT NOT NULL, role_id TEXT NOT NULL, verified_at REAL NOT NULL, \
     PRIMARY KEY (discord_id, guild_id, role_id))",
    // 网页登录时 Discord 给的名字 / 头像：没领过 Key 的人（没有 discord_registrations 行）首页也能显示自己。
    // 长期保留。
    "CREATE TABLE IF NOT EXISTS web_profiles (\
     discord_id TEXT PRIMARY KEY, username TEXT NOT NULL DEFAULT '', display_name TEXT NOT NULL DEFAULT '', \
     avatar TEXT NOT NULL DEFAULT '', updated_at REAL NOT NULL)",
    // 删 Key 前留下的归属（Key 编号 → Discord 账号 + 显示名）：闲置回收 / 撤销 / 站长删除都经过 `delete_key`。
    // 删掉的 Key 在用量日志里只剩编号，周报靠它把这些行归到账号上。长期保留。
    // discord_id 可能为 NULL（2.x 时期没登记账号的 Key）。
    "CREATE TABLE IF NOT EXISTS deleted_key_owners (\
     key_id INTEGER PRIMARY KEY, discord_id TEXT, name TEXT NOT NULL DEFAULT '', deleted_at REAL NOT NULL)",
    // 网页登录时的浏览器设备指纹（前端 Canvas/WebGL/屏幕/时区等合成的短哈希）：同一个设备指纹下登录了
    // 多个 Discord 账号 = 疑似同一个人的小号。防滥用用途，长期保留。
    "CREATE TABLE IF NOT EXISTS device_logins (\
     discord_id TEXT NOT NULL, device_id TEXT NOT NULL, ip TEXT NOT NULL DEFAULT '', ts REAL NOT NULL, \
     PRIMARY KEY (discord_id, device_id))",
    "CREATE INDEX IF NOT EXISTS idx_device_logins_dev ON device_logins (device_id)",
    // 「每位成员到网页登录一次」开关按 (discord_id, ts) 查：每个出图请求都会查一次
    "CREATE INDEX IF NOT EXISTS idx_device_logins_who ON device_logins (discord_id, ts)",
    "CREATE TABLE IF NOT EXISTS usage_request_links (request_id TEXT PRIMARY KEY, usage_log_id INTEGER NOT NULL UNIQUE)",
    // 设备指纹的可读分项（显卡 / 屏幕 / 时区 / 硬件 …）：device_id = 各项合成的短哈希，data 是 JSON 明细，
    // 站长查小号时能看清「哪几项一样」而不是只有一个哈希。同样只留 30 天。
    "CREATE TABLE IF NOT EXISTS device_fingerprints (\
     device_id TEXT PRIMARY KEY, data TEXT NOT NULL DEFAULT '', ts REAL NOT NULL)",
    // 每个 Discord 账号最近一次网页登录（永久保留，不随 device_logins 的 30 天清理）：
    // 「每位成员网页登录一次」开关按它放行。fp = 这次登录的设备指纹状态：
    // ok（服务端认得的指纹）/ none（没交指纹：脚本被拦、直接打开 /login）/ unknown（交了服务端不认得的指纹 = 疑似伪造）。
    "CREATE TABLE IF NOT EXISTS web_logins (\
     discord_id TEXT PRIMARY KEY, first_ts REAL NOT NULL, last_ts REAL NOT NULL, \
     fp TEXT NOT NULL DEFAULT '', ip TEXT NOT NULL DEFAULT '')",
    // 反滥用待审队列：每日扫描 / 注册 / 登录产生的可疑信号，按 (kind, subject) 去重更新，站长在后台复核，不自动封号。
    // subject = 涉及的 Discord ID 排序后用逗号连起来；score 越高越可疑；detail 是证据 JSON；
    // status：open（待审）/ dismissed（忽略，同样的证据不再提醒，除非分数变高）/ confirmed（确认，已打标签）。
    "CREATE TABLE IF NOT EXISTS abuse_signals (\
     id INTEGER PRIMARY KEY AUTOINCREMENT, kind TEXT NOT NULL, subject TEXT NOT NULL, \
     key_ids TEXT NOT NULL DEFAULT '', score REAL NOT NULL DEFAULT 0, detail TEXT NOT NULL DEFAULT '', \
     status TEXT NOT NULL DEFAULT 'open', first_ts REAL NOT NULL, last_ts REAL NOT NULL, \
     UNIQUE (kind, subject))",
    "CREATE INDEX IF NOT EXISTS idx_abuse_signals_status ON abuse_signals (status, score)",
    // 浏览器标记：第一次打开本站时种在浏览器里的长期随机 ID（Cookie + localStorage 各存一份）。
    // 同一个浏览器换 Discord 账号登录，标记一模一样 = 几乎可以确定是同一个人。防滥用用途，只留 90 天。
    "CREATE TABLE IF NOT EXISTS browser_logins (\
     discord_id TEXT NOT NULL, browser_id TEXT NOT NULL, ip TEXT NOT NULL DEFAULT '', ts REAL NOT NULL, \
     PRIMARY KEY (discord_id, browser_id))",
    "CREATE INDEX IF NOT EXISTS idx_browser_logins_bid ON browser_logins (browser_id)",
    // 专项问卷（已下线，表只为保留旧数据，代码不再读写；删 Key 时顺带清掉这位成员的行）：站长对某一位成员单独发的几道问题。
    // questions = JSON [{label, hint, required}]；answers = JSON 字符串数组（与 questions 一一对应），未回答为空串；
    // answered_ts 为 NULL = 待回答。一次性：回答后不能再改。
    "CREATE TABLE IF NOT EXISTS member_inquiries (\
     id INTEGER PRIMARY KEY AUTOINCREMENT, key_id INTEGER NOT NULL, title TEXT NOT NULL, \
     intro TEXT NOT NULL DEFAULT '', questions TEXT NOT NULL, answers TEXT NOT NULL DEFAULT '', \
     created_ts REAL NOT NULL, answered_ts REAL, by TEXT NOT NULL DEFAULT '')",
    "CREATE INDEX IF NOT EXISTS idx_member_inquiries_key ON member_inquiries (key_id, answered_ts)",
    // 请求日志按来源 IP 精确筛（看同一个 IP 用过哪些 Key）。部分索引：保留期外的行 src 是空串，不进索引。
    "CREATE INDEX IF NOT EXISTS idx_log_src ON usage_log (src) WHERE src!=''",
    // 网页登录时（OAuth scope `guilds`）读到的成员所在服务器列表（站长 10-23：防一人多号 / 共用 Key）。
    // 每个 (账号, 服务器) 一行，first_ts / last_ts 是第一次 / 最近一次登录时看到它；退出的服务器保留不删。永久保留。
    "CREATE TABLE IF NOT EXISTS discord_user_guilds (\
     discord_id TEXT NOT NULL, guild_id TEXT NOT NULL, name TEXT NOT NULL DEFAULT '', \
     first_ts REAL NOT NULL, last_ts REAL NOT NULL, PRIMARY KEY (discord_id, guild_id))",
    "CREATE INDEX IF NOT EXISTS idx_discord_user_guilds_guild ON discord_user_guilds (guild_id)",
    // 网页登录时（OAuth scope `connections`）读到的绑定外部账号（Steam / GitHub / Twitch …）：
    // 同一个外部账号绑在多个 Discord 账号上 = 几乎可以确定是同一个人。type + ext_id 是外部平台和平台上的 ID。永久保留。
    "CREATE TABLE IF NOT EXISTS discord_user_connections (\
     discord_id TEXT NOT NULL, type TEXT NOT NULL, ext_id TEXT NOT NULL, name TEXT NOT NULL DEFAULT '', \
     verified INTEGER NOT NULL DEFAULT 0, first_ts REAL NOT NULL, last_ts REAL NOT NULL, \
     PRIMARY KEY (discord_id, type, ext_id))",
    "CREATE INDEX IF NOT EXISTS idx_discord_user_connections_ext ON discord_user_connections (type, ext_id)",
    // 设备指纹的补充项（站长 10-23）：语音列表 / 字体探测 / 色域 / HDR / 像素比 / 存储配额档位 / UA-CH 高熵值的哈希或原值。
    // 和 device_fingerprints 分开存，指纹 id 的算法不变（已有的关联不受影响）。按 (设备指纹, 浏览器标记) 一行，
    // data 是规整后的 JSON 明细，xid 是它的 sha256 前 16 位（比对用）。battery_* 是同一次上报的电量快照，登录时转存到
    // device_battery。永久保留。
    "CREATE TABLE IF NOT EXISTS device_fp_extra (\
     device_id TEXT NOT NULL, browser_id TEXT NOT NULL DEFAULT '', data TEXT NOT NULL DEFAULT '', \
     xid TEXT NOT NULL DEFAULT '', battery_level REAL, battery_charging INTEGER, battery_ts REAL, ts REAL NOT NULL, \
     PRIMARY KEY (device_id, browser_id))",
    "CREATE INDEX IF NOT EXISTS idx_device_fp_extra_xid ON device_fp_extra (xid)",
    // 每次网页登录时的电量快照（level 0–1，charging 0/1）：两个账号在 30 分钟内电量和充电状态一模一样 =
    // 很可能是同一台设备上的两个浏览器。永久保留。
    "CREATE TABLE IF NOT EXISTS device_battery (\
     discord_id TEXT NOT NULL, ts REAL NOT NULL, level REAL NOT NULL, charging INTEGER NOT NULL)",
    "CREATE INDEX IF NOT EXISTS idx_device_battery_ts ON device_battery (ts)",
    // 成员上传的参考图（站长 10-23：图生图 / Vibe / 精确参考 / Vibe 编码）的哈希：sha256（原始字节）和
    // 64 位 dHash（解码后 9x8 灰度差分，16 位十六进制；解不出图为 NULL）。不存图片本身。
    // 不同 Key 用了同一张（或几乎一样的）参考图 = 关联证据。rid 同 usage_log.rid。永久保留。
    "CREATE TABLE IF NOT EXISTS ref_images (\
     rid TEXT NOT NULL DEFAULT '', key_id INTEGER NOT NULL, ts REAL NOT NULL, kind TEXT NOT NULL, \
     sha256 TEXT NOT NULL, dhash TEXT)",
    "CREATE INDEX IF NOT EXISTS idx_ref_images_sha ON ref_images (sha256)",
    "CREATE INDEX IF NOT EXISTS idx_ref_images_ts ON ref_images (ts)",
];

/// 给 3.0 新增的表补列（在 `RUST_ADDED_TABLES` 之后执行；老库已有时报 duplicate column，忽略）。
pub const RUST_ADDED_TABLE_COLUMNS: &[&str] = &[
    "ALTER TABLE request_settlements ADD COLUMN month TEXT NOT NULL DEFAULT ''",
    "ALTER TABLE request_settlements ADD COLUMN is_vibe INTEGER NOT NULL DEFAULT 0",
    "ALTER TABLE request_settlements ADD COLUMN resolution_note TEXT NOT NULL DEFAULT ''",
    "ALTER TABLE request_settlements ADD COLUMN result_at REAL",
    "ALTER TABLE request_settlements ADD COLUMN delivery_state TEXT NOT NULL DEFAULT 'pending'",
    "ALTER TABLE request_settlements ADD COLUMN correlation_id TEXT NOT NULL DEFAULT ''",
    // 这个账号第一次在这台设备 / 这个浏览器上网页登录的时刻（ts 是最近一次）。待审卡片显示「首次 / 最近登录」用；
    // 加列之前的老行为 NULL，读的时候退回 ts。
    "ALTER TABLE device_logins ADD COLUMN first_ts REAL",
    "ALTER TABLE browser_logins ADD COLUMN first_ts REAL",
];

/// 删除凭据只清理额度重置基线；历史计数与归属保留，不再归档权限豁免。
const REBUILD_TRIGGER: &str = "DROP TRIGGER IF EXISTS preserve_deleted_key_usage;
    DROP TRIGGER IF EXISTS prevent_deleted_key_quota_reset;
    CREATE TRIGGER preserve_deleted_key_usage BEFORE DELETE ON api_keys BEGIN
    DELETE FROM daily_quota_offsets WHERE key_id=OLD.id; END;
    CREATE TRIGGER prevent_deleted_key_quota_reset BEFORE INSERT ON daily_quota_offsets
    WHEN NOT EXISTS(SELECT 1 FROM api_keys WHERE id=NEW.key_id) BEGIN SELECT RAISE(IGNORE); END;";

const INDEXES: &[&str] = &[
    "CREATE INDEX IF NOT EXISTS idx_log_ts_upstatus ON usage_log (ts, up_status)",
    "CREATE INDEX IF NOT EXISTS idx_log_key_id ON usage_log (key_id, id DESC)",
    "CREATE INDEX IF NOT EXISTS idx_log_rid ON usage_log (rid)",
    "CREATE INDEX IF NOT EXISTS idx_admin_actions_action ON admin_actions (action)",
];

pub const SCHEMA_VERSION: i64 = 1;

/// 连接级设置：WAL + synchronous=NORMAL + 锁等待 15 秒。
pub fn configure(conn: &Connection) -> rusqlite::Result<()> {
    conn.busy_timeout(std::time::Duration::from_secs(15))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "OFF")?;
    Ok(())
}

fn is_duplicate_column(e: &rusqlite::Error) -> bool {
    matches!(e, rusqlite::Error::SqliteFailure(f, Some(msg))
        if f.code == ErrorCode::Unknown && msg.contains("duplicate column"))
}

fn has_column(conn: &Connection, table: &str, column: &str) -> rusqlite::Result<bool> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let names = stmt.query_map([], |r| r.get::<_, String>(1))?;
    for n in names {
        if n? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

/// 已下线功能的表：启动时顺手清掉（建表语句已不在 `schema.sql`，线上库里的旧表留着无用）。
/// `error_events` 是删掉的 Bug 追踪（10-27）：表里的历史错误随表一起删除。
pub const REMOVED_TABLES: &[&str] = &["member_survey", "feedback", "error_events"];

/// 建表 + 迁移。
pub fn run(conn: &mut Connection) -> rusqlite::Result<()> {
    configure(conn)?;
    conn.execute_batch(SCHEMA)?;
    for t in REMOVED_TABLES {
        conn.execute(&format!("DROP TABLE IF EXISTS {t}"), [])?;
    }
    for ddl in ADD_COLUMNS {
        match conn.execute(ddl, []) {
            Ok(_) => {}
            Err(e) if is_duplicate_column(&e) => {}
            Err(e) => return Err(e),
        }
    }
    conn.execute_batch(REBUILD_TRIGGER)?;
    if !has_column(conn, "api_keys", "discord_id")? {
        conn.execute("ALTER TABLE api_keys ADD COLUMN discord_id TEXT", [])?;
    }
    // 3.0 的列放在 2.x 的所有列（含上面的 discord_id）之后：全新库和线上库的列顺序一致，
    // 不带列名的整行 `INSERT ... VALUES(...)` 也不受影响。
    for ddl in RUST_ADDED_COLUMNS {
        match conn.execute(ddl, []) {
            Ok(_) => {}
            Err(e) if is_duplicate_column(&e) => {}
            Err(e) => return Err(e),
        }
    }
    for ddl in RUST_ADDED_TABLES {
        conn.execute(ddl, [])?;
    }
    for ddl in RUST_ADDED_TABLE_COLUMNS {
        match conn.execute(ddl, []) {
            Ok(_) => {}
            Err(e) if is_duplicate_column(&e) => {}
            Err(e) => return Err(e),
        }
    }
    // 两条更老的一次性迁移（unconfirmed_anlas / legacy_free_images）只对 2026-10 之前的库有意义：
    // 当前建表语句已含这两列，线上库也早已迁过。真遇到缺列的古董库就直接报错，不在这里重放历史。
    for (table, col) in [
        ("usage_log", "unconfirmed_anlas"),
        ("counters", "legacy_free_images"),
    ] {
        if !has_column(conn, table, col)? {
            return Err(rusqlite::Error::InvalidColumnName(format!(
                "{table}.{col}：库太旧，请先用 2.x 版启动一次完成迁移"
            )));
        }
    }
    for idx in INDEXES {
        conn.execute(idx, [])?;
    }
    conn.execute_batch(STATUS_SCHEMA)?;
    conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    Ok(())
}
