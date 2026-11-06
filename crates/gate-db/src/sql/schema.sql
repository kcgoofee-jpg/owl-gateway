-- 3.0 数据库结构：唯一形态。
--
-- 这个文件就是最终形态，不再有任何「先建 2.x 的表再 ALTER 补列」的路径。
-- 老的兼容层（ADD_COLUMNS / policy_migration / archive_removal）已经删除，
-- 库不是这个形态时 bootstrap 会直接报错，不做在线迁移。
--
-- 行尾注释会被原样存进 sqlite_master（tests/schema_parity.rs 逐字对拍），改结构时一并改。

CREATE TABLE IF NOT EXISTS api_keys (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL DEFAULT '',
    token TEXT NOT NULL UNIQUE,
    enabled INTEGER NOT NULL DEFAULT 1,
    expires_at REAL,
    created_at REAL NOT NULL,
    last_used_at REAL,
    last_image_at REAL,          -- 最近一次成功生图，查额度或图片工具不算活跃
    discord_id TEXT,
    disabled_reason TEXT         -- gate_core::key_status；启用中为 NULL
);

CREATE TABLE IF NOT EXISTS site_settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS discord_registrations (
    discord_id TEXT PRIMARY KEY,
    key_id INTEGER NOT NULL UNIQUE,
    created_at REAL NOT NULL,
    role_granted INTEGER NOT NULL DEFAULT 0,
    username TEXT,
    display_name TEXT,
    avatar TEXT,
    member_checked_at REAL NOT NULL DEFAULT 0,
    guild_joined_at REAL,        -- Discord 返回的 joined_at（Unix 秒；没取到为 NULL）
    member_flags INTEGER         -- 1 = 退群后又重新加入
);

CREATE TABLE IF NOT EXISTS discord_bans (
    discord_id TEXT PRIMARY KEY,
    created_at REAL NOT NULL,
    reason TEXT NOT NULL DEFAULT '',
    username TEXT NOT NULL DEFAULT '',
    display_name TEXT NOT NULL DEFAULT '',
    avatar TEXT NOT NULL DEFAULT '',
    banned_by TEXT NOT NULL DEFAULT '系统'
);

CREATE TABLE IF NOT EXISTS pending_role_removals (
    discord_id TEXT PRIMARY KEY,
    created_at REAL NOT NULL
);

CREATE TABLE IF NOT EXISTS counters (
    key_id INTEGER NOT NULL,
    day TEXT NOT NULL,            -- YYYY-MM-DD (按配置时区)
    images INTEGER NOT NULL DEFAULT 0,
    legacy_free_images INTEGER NOT NULL DEFAULT 0,   -- legacy = V4.5 及更早模型，不是遗留代码
    anlas REAL NOT NULL DEFAULT 0,
    text_tokens INTEGER NOT NULL DEFAULT 0,
    requests INTEGER NOT NULL DEFAULT 0,
    v5 INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (key_id, day)
);

-- client = 原始 User-Agent（截到 160 字符），src = 来源 IP 原文（不打码）。
-- 两者一律保留，没有「到期清空」这回事（旧的 usage_origin_keep_days 设置已删除）。
CREATE TABLE IF NOT EXISTS usage_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    ts REAL NOT NULL,
    key_id INTEGER,
    key_name TEXT NOT NULL DEFAULT '',
    kind TEXT NOT NULL,           -- image / text / voice / tags / chat
    model TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL,         -- ok / error / rejected
    images INTEGER NOT NULL DEFAULT 0,
    anlas REAL NOT NULL DEFAULT 0,
    tokens INTEGER NOT NULL DEFAULT 0,
    unconfirmed_anlas REAL NOT NULL DEFAULT 0,
    detail TEXT NOT NULL DEFAULT '',
    wait_ms INTEGER NOT NULL DEFAULT 0,   -- 从收到请求到发往上游（排队 + 冷却）
    dur_ms INTEGER NOT NULL DEFAULT 0,    -- 上游处理耗时；没发到上游为 0
    client TEXT NOT NULL DEFAULT '',      -- User-Agent 摘要，客户端自报，仅供参考
    up_status INTEGER NOT NULL DEFAULT 0, -- 上游最后返回的 HTTP 状态码；没收到响应为 0
    rid TEXT NOT NULL DEFAULT '',         -- 请求编号（响应头 X-Request-Id），成员报错时据此定位
    src TEXT NOT NULL DEFAULT '',         -- 来源 IP 原文
    reason TEXT NOT NULL DEFAULT '',      -- 拒绝原因码（gate_core::reasons）：统计按它数，不再 LIKE 中文 detail
    token_id TEXT                         -- 这条请求最后由哪个上游账号处理；鉴权 / 准入阶段被拒为 NULL
);
CREATE INDEX IF NOT EXISTS idx_log_ts ON usage_log (ts DESC);
CREATE INDEX IF NOT EXISTS idx_log_key ON usage_log (key_id, ts DESC);
CREATE INDEX IF NOT EXISTS idx_log_ts_upstatus ON usage_log (ts, up_status);
CREATE INDEX IF NOT EXISTS idx_log_key_id ON usage_log (key_id, id DESC);
CREATE INDEX IF NOT EXISTS idx_log_rid ON usage_log (rid);
CREATE INDEX IF NOT EXISTS idx_log_src ON usage_log (src);
-- 后台成员列表的合并聚合（首末出图时间 + 累计张数，见 member_image_stats）：覆盖索引避免整表回读
CREATE INDEX IF NOT EXISTS idx_log_image_stats ON usage_log (status, kind, key_id, ts, images);

CREATE TABLE IF NOT EXISTS upstream_token_counters (
    token_id TEXT NOT NULL,
    day TEXT NOT NULL,            -- YYYY-MM-DD (按配置时区)
    images INTEGER NOT NULL DEFAULT 0,
    v5 INTEGER NOT NULL DEFAULT 0,
    units REAL,
    PRIMARY KEY (token_id, day)
);

CREATE TABLE IF NOT EXISTS key_tags (          -- 后台手动标签（只给站长看，成员看不到）
    key_id INTEGER NOT NULL,
    tag TEXT NOT NULL,
    note TEXT NOT NULL DEFAULT '',
    by TEXT NOT NULL DEFAULT '',
    ts REAL NOT NULL,
    PRIMARY KEY (key_id, tag)
);

CREATE TABLE IF NOT EXISTS key_donations (     -- 访客在首页「捐赠 Key」提交的号（后台人工验证后才接入上游，不自动生效）
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    ts REAL NOT NULL,
    token TEXT NOT NULL,                        -- NovelAI Token 原文：只进这张表，不写日志
    message TEXT NOT NULL DEFAULT '',           -- 捐赠者的留言 / 使用要求（必填）
    contact TEXT NOT NULL DEFAULT '',           -- 选填联系方式
    handled INTEGER NOT NULL DEFAULT 0,
    handled_at REAL,
    handled_note TEXT NOT NULL DEFAULT ''
);

CREATE TABLE IF NOT EXISTS share_state (       -- Key 临时暂停
    key_id INTEGER PRIMARY KEY,
    paused_until REAL NOT NULL DEFAULT 0,
    pause_reason TEXT NOT NULL DEFAULT ''
);

CREATE TABLE IF NOT EXISTS upstream_snapshots (   -- 每小时记一次上游真实状态，和我们自己的计数做独立核对
    ts REAL NOT NULL,
    v5_percent REAL,              -- 账号 V5 剩余 %（订阅接口）
    v5_rate REAL,                 -- 实测恢复 %/天（拿不到时为空）
    anlas REAL,                   -- 账号 Anlas 余额
    fresh INTEGER NOT NULL DEFAULT 0,   -- 1 = 这次是主动刚读的（不是出图时留下的旧缓存），才能拿来算恢复速度
    token_id TEXT NOT NULL DEFAULT ''   -- 每个上游账号各一行
);

CREATE TABLE IF NOT EXISTS coupons (       -- 成员的券（现在只有「重置券」：在首页自己点一下，重置当天的 V5 / Anlas 额度）
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    key_id INTEGER NOT NULL,
    kind TEXT NOT NULL DEFAULT 'reset',
    note TEXT NOT NULL DEFAULT '',       -- 发券理由，成员能看到（如「唱得真好听」）
    by TEXT NOT NULL DEFAULT '',
    created_at REAL NOT NULL,
    expires_at REAL NOT NULL,
    used_at REAL,
    scope TEXT NOT NULL DEFAULT 'v45,v5,anlas'   -- 逗号分隔的重置项（v45 / v5 / anlas）
);
CREATE INDEX IF NOT EXISTS idx_coupons_key ON coupons(key_id, used_at);

CREATE TABLE IF NOT EXISTS admin_actions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    ts REAL NOT NULL,
    actor TEXT NOT NULL,          -- 后台(打码 IP) / Discord:<id> / 系统
    action TEXT NOT NULL,
    target TEXT NOT NULL DEFAULT '',
    detail TEXT NOT NULL DEFAULT '',
    ok INTEGER NOT NULL DEFAULT 1
);
CREATE INDEX IF NOT EXISTS idx_admin_actions_ts ON admin_actions (ts DESC);
CREATE INDEX IF NOT EXISTS idx_admin_actions_action ON admin_actions (action);

CREATE TABLE IF NOT EXISTS upstream_token_settings (
    token_id TEXT PRIMARY KEY,
    v5_daily_limit INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS upstream_token_enabled (
    token_id TEXT PRIMARY KEY,
    enabled INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE IF NOT EXISTS daily_quota_offsets (
    key_id INTEGER NOT NULL,
    day TEXT NOT NULL,
    anlas REAL NOT NULL DEFAULT 0,
    v5 INTEGER NOT NULL DEFAULT 0,
    legacy INTEGER NOT NULL DEFAULT 0,   -- V4.5 免费图的基线
    PRIMARY KEY (key_id, day)
);

CREATE TRIGGER IF NOT EXISTS preserve_deleted_key_usage
BEFORE DELETE ON api_keys
BEGIN
    DELETE FROM daily_quota_offsets WHERE key_id=OLD.id;
END;
CREATE TRIGGER IF NOT EXISTS prevent_deleted_key_quota_reset
BEFORE INSERT ON daily_quota_offsets
WHEN NOT EXISTS (SELECT 1 FROM api_keys WHERE id=NEW.key_id)
BEGIN
    SELECT RAISE(IGNORE);
END;

CREATE TABLE IF NOT EXISTS key_keystores (   -- 官网生图页的本地历史加密密钥库（客户端加密后的不透明密文）
    key_id INTEGER PRIMARY KEY,
    blob TEXT NOT NULL,
    change_index INTEGER NOT NULL DEFAULT 0,
    updated_at REAL NOT NULL
);

CREATE TABLE IF NOT EXISTS key_settings (    -- 官网生图页的客户端设置（主题/模型/许可标记等，整块 JSON）
    key_id INTEGER PRIMARY KEY,
    json TEXT NOT NULL,
    updated_at REAL NOT NULL
);

CREATE TABLE IF NOT EXISTS gateway_health_probe (id INTEGER PRIMARY KEY, bit INTEGER NOT NULL);

CREATE TABLE IF NOT EXISTS member_rules (    -- 成员规则例外（默认值在 site_settings）
    discord_id TEXT PRIMARY KEY, overrides TEXT NOT NULL, recycle_not_before REAL);

-- 图片请求的持久生命周期。未知的已派发结果留作核对，绝不从这张表重放上游请求。
CREATE TABLE IF NOT EXISTS request_settlements (
    request_id TEXT PRIMARY KEY,
    rid TEXT NOT NULL,
    key_id INTEGER NOT NULL,
    status TEXT NOT NULL,
    day TEXT NOT NULL,
    anlas REAL NOT NULL DEFAULT 0,
    v5 INTEGER NOT NULL DEFAULT 0,
    legacy INTEGER NOT NULL DEFAULT 0,
    dispatched_at REAL,
    result_json TEXT,
    updated_at REAL NOT NULL,
    month TEXT NOT NULL DEFAULT '',
    is_vibe INTEGER NOT NULL DEFAULT 0,
    resolution_note TEXT NOT NULL DEFAULT '',
    result_at REAL,
    delivery_state TEXT NOT NULL DEFAULT 'pending',
    correlation_id TEXT NOT NULL DEFAULT '',
    site_paid_token TEXT NOT NULL DEFAULT '',
    site_paid_hold REAL NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_settlement_site_paid ON request_settlements(site_paid_token) WHERE site_paid_hold > 0;

-- 成员用 Discord OAuth 授权后核验到的身份组（领 Key 的门槛只认这张表）。
CREATE TABLE IF NOT EXISTS role_verifications (
    discord_id TEXT NOT NULL, guild_id TEXT NOT NULL, role_id TEXT NOT NULL, verified_at REAL NOT NULL,
    PRIMARY KEY (discord_id, guild_id, role_id));

-- 网页登录时 Discord 给的名字 / 头像：没领过 Key 的人首页也能显示自己。
CREATE TABLE IF NOT EXISTS web_profiles (
    discord_id TEXT PRIMARY KEY, username TEXT NOT NULL DEFAULT '', display_name TEXT NOT NULL DEFAULT '',
    avatar TEXT NOT NULL DEFAULT '', updated_at REAL NOT NULL);

-- 删 Key 前留下的归属（Key 编号 → Discord 账号 + 显示名）。
CREATE TABLE IF NOT EXISTS deleted_key_owners (
    key_id INTEGER PRIMARY KEY, discord_id TEXT, name TEXT NOT NULL DEFAULT '', deleted_at REAL NOT NULL);

-- 网页登录时的浏览器设备指纹（前端 Canvas/WebGL/屏幕/时区等合成的短哈希）。
CREATE TABLE IF NOT EXISTS device_logins (
    discord_id TEXT NOT NULL, device_id TEXT NOT NULL, ip TEXT NOT NULL DEFAULT '', ts REAL NOT NULL, first_ts REAL,
    PRIMARY KEY (discord_id, device_id));
CREATE INDEX IF NOT EXISTS idx_device_logins_dev ON device_logins (device_id);
CREATE INDEX IF NOT EXISTS idx_device_logins_who ON device_logins (discord_id, ts);

CREATE TABLE IF NOT EXISTS usage_request_links (request_id TEXT PRIMARY KEY, usage_log_id INTEGER NOT NULL UNIQUE);

-- 设备指纹的可读分项（显卡 / 屏幕 / 时区 / 硬件 …）：data 是 JSON 明细。
CREATE TABLE IF NOT EXISTS device_fingerprints (
    device_id TEXT PRIMARY KEY, data TEXT NOT NULL DEFAULT '', ts REAL NOT NULL);

-- 每个 Discord 账号最近一次网页登录。fp = 这次登录的设备指纹状态：
-- ok（服务端认得的指纹）/ none（没交指纹）/ unknown（交了服务端不认得的指纹）。
CREATE TABLE IF NOT EXISTS web_logins (
    discord_id TEXT PRIMARY KEY, first_ts REAL NOT NULL, last_ts REAL NOT NULL,
    fp TEXT NOT NULL DEFAULT '', ip TEXT NOT NULL DEFAULT '');

-- 浏览器标记：第一次打开本站时种在浏览器里的长期随机 ID。
CREATE TABLE IF NOT EXISTS browser_logins (
    discord_id TEXT NOT NULL, browser_id TEXT NOT NULL, ip TEXT NOT NULL DEFAULT '', ts REAL NOT NULL, first_ts REAL,
    PRIMARY KEY (discord_id, browser_id));
CREATE INDEX IF NOT EXISTS idx_browser_logins_bid ON browser_logins (browser_id);

-- 网页登录时（OAuth scope `guilds`）读到的成员所在服务器列表。
CREATE TABLE IF NOT EXISTS discord_user_guilds (
    discord_id TEXT NOT NULL, guild_id TEXT NOT NULL, name TEXT NOT NULL DEFAULT '',
    first_ts REAL NOT NULL, last_ts REAL NOT NULL, PRIMARY KEY (discord_id, guild_id));
CREATE INDEX IF NOT EXISTS idx_discord_user_guilds_guild ON discord_user_guilds (guild_id);

-- 网页登录时（OAuth scope `connections`）读到的绑定外部账号（Steam / GitHub / Twitch …）。
CREATE TABLE IF NOT EXISTS discord_user_connections (
    discord_id TEXT NOT NULL, type TEXT NOT NULL, ext_id TEXT NOT NULL, name TEXT NOT NULL DEFAULT '',
    verified INTEGER NOT NULL DEFAULT 0, first_ts REAL NOT NULL, last_ts REAL NOT NULL,
    PRIMARY KEY (discord_id, type, ext_id));
CREATE INDEX IF NOT EXISTS idx_discord_user_connections_ext ON discord_user_connections (type, ext_id);

-- 设备指纹的补充项：语音列表 / 字体探测 / 色域 / HDR / 像素比 / 存储配额档位 / UA-CH 高熵值。
CREATE TABLE IF NOT EXISTS device_fp_extra (
    device_id TEXT NOT NULL, browser_id TEXT NOT NULL DEFAULT '', data TEXT NOT NULL DEFAULT '',
    xid TEXT NOT NULL DEFAULT '', battery_level REAL, battery_charging INTEGER, battery_ts REAL, ts REAL NOT NULL,
    PRIMARY KEY (device_id, browser_id));
CREATE INDEX IF NOT EXISTS idx_device_fp_extra_xid ON device_fp_extra (xid);

-- 每次网页登录时的电量快照（level 0–1，charging 0/1）。
CREATE TABLE IF NOT EXISTS device_battery (
    discord_id TEXT NOT NULL, ts REAL NOT NULL, level REAL NOT NULL, charging INTEGER NOT NULL);
CREATE INDEX IF NOT EXISTS idx_device_battery_ts ON device_battery (ts);

-- 成员上传的参考图（图生图 / Vibe / 精确参考 / Vibe 编码）的哈希：sha256 和 64 位 dHash。不存图片本身。
CREATE TABLE IF NOT EXISTS ref_images (
    rid TEXT NOT NULL DEFAULT '', key_id INTEGER NOT NULL, ts REAL NOT NULL, kind TEXT NOT NULL,
    sha256 TEXT NOT NULL, dhash TEXT);
CREATE INDEX IF NOT EXISTS idx_ref_images_sha ON ref_images (sha256);
CREATE INDEX IF NOT EXISTS idx_ref_images_ts ON ref_images (ts);
