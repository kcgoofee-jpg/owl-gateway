CREATE TABLE IF NOT EXISTS api_keys (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL DEFAULT '',
    token TEXT NOT NULL UNIQUE,
    enabled INTEGER NOT NULL DEFAULT 1,
    daily_images INTEGER NOT NULL DEFAULT 100,
    daily_anlas REAL NOT NULL DEFAULT 0,
    daily_v5 INTEGER NOT NULL DEFAULT 0,
    monthly_anlas REAL NOT NULL DEFAULT 500,
    daily_text_tokens INTEGER NOT NULL DEFAULT 150000,
    rpm INTEGER NOT NULL DEFAULT 10,
    allow_anlas INTEGER NOT NULL DEFAULT 0,
    allow_img2img INTEGER NOT NULL DEFAULT 0,
    exclude_global_v5 INTEGER NOT NULL DEFAULT 0,
    image_model_scope TEXT NOT NULL DEFAULT 'legacy',
    is_admin INTEGER NOT NULL DEFAULT 0,
    is_test INTEGER NOT NULL DEFAULT 0,   -- 测试 Key：不计入成员统计、上游表现，不会被闲置回收
    anlas_auto INTEGER NOT NULL DEFAULT 0,
    quota_auto INTEGER NOT NULL DEFAULT 1,   -- 1 额度由动态额度算法管理（quota_algo.py）；-1 站长手动 -- 1 自动分配管理（只用于 V5 续杯）；0 交给算法；-1 站长手动（关闭或手动额度）
    expires_at REAL,
    created_at REAL NOT NULL,
    last_used_at REAL,
    last_image_at REAL   -- 最后一次成功生图（含放大 / 导演 / Vibe）：闲置回收只看它，查额度不算活跃（2026-10-13 站长）
);
CREATE TABLE IF NOT EXISTS site_settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS anlas_reconciliations (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    snapshot TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS discord_registrations (
    discord_id TEXT PRIMARY KEY,
    key_id INTEGER NOT NULL UNIQUE,
    created_at REAL NOT NULL
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
    legacy_free_images INTEGER NOT NULL DEFAULT 0,
    anlas REAL NOT NULL DEFAULT 0,
    text_tokens INTEGER NOT NULL DEFAULT 0,
    requests INTEGER NOT NULL DEFAULT 0,
    v5 INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (key_id, day)
);
CREATE TABLE IF NOT EXISTS generation_audit (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    ts REAL NOT NULL,
    key_id INTEGER,
    key_name TEXT NOT NULL DEFAULT '',
    kind TEXT NOT NULL DEFAULT 'image',
    model TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL DEFAULT '',
    prompt TEXT NOT NULL DEFAULT '',
    negative TEXT NOT NULL DEFAULT '',
    extra TEXT NOT NULL DEFAULT '',
    thumb BLOB,
    image BLOB,
    image_type TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS idx_audit_ts ON generation_audit(ts);
CREATE INDEX IF NOT EXISTS idx_audit_key ON generation_audit(key_id, ts);
CREATE INDEX IF NOT EXISTS idx_audit_key_id ON generation_audit(key_id, id DESC);
-- usage_log 表内的行尾注释会原样存进 sqlite_master（与旧版逐字一致，见 tests/schema_parity.rs），所以不改；
-- 以这里为准：client = 原始 User-Agent（截到 160 字符），src = 来源 IP 原文（不打码），
-- 两者按 usage_origin_keep_days（后台可调）保留，到期由维护任务清成空串（Db::purge_usage_origin）。
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
    reason TEXT NOT NULL DEFAULT '',      -- 拒绝原因码（app/reasons.py）：统计和自动驾驶按它数，不再 LIKE 中文 detail
    wait_ms INTEGER NOT NULL DEFAULT 0,   -- 从收到请求到发往上游（排队 + 冷却）
    dur_ms INTEGER NOT NULL DEFAULT 0,    -- 上游处理耗时；没发到上游为 0
    client TEXT NOT NULL DEFAULT '',      -- User-Agent 摘要，客户端自报，仅供参考
    up_status INTEGER NOT NULL DEFAULT 0, -- 上游最后返回的 HTTP 状态码；没收到响应为 0
    rid TEXT NOT NULL DEFAULT '',         -- 请求编号（响应头 X-Request-Id），成员报错时据此定位
    src TEXT NOT NULL DEFAULT '',         -- 来源网络打码标签（如 120.235.*.*），防分享溯源用，不存完整 IP
    ver TEXT NOT NULL DEFAULT ''          -- 网关版本（规则版本）：回测按参数版本分段，避免把不同规则下的数据混在一起
);
CREATE INDEX IF NOT EXISTS idx_log_ts ON usage_log (ts DESC);
CREATE INDEX IF NOT EXISTS idx_log_key ON usage_log (key_id, ts DESC);
CREATE TABLE IF NOT EXISTS upstream_token_counters (
    token_id TEXT NOT NULL,
    day TEXT NOT NULL,            -- YYYY-MM-DD (按配置时区)
    images INTEGER NOT NULL DEFAULT 0,
    v5 INTEGER NOT NULL DEFAULT 0,
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
CREATE TABLE IF NOT EXISTS member_survey (    -- 成员详细问卷（/survey 网页，按 Key 识别，一人一份可覆盖；Discord /反馈 弹窗走 feedback 表）
    key_id INTEGER PRIMARY KEY,
    client TEXT NOT NULL DEFAULT '',
    understand TEXT NOT NULL DEFAULT '',
    improve TEXT NOT NULL DEFAULT '',
    improve_other TEXT NOT NULL DEFAULT '',
    donate TEXT NOT NULL DEFAULT '',
    ts REAL NOT NULL
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
CREATE TABLE IF NOT EXISTS share_state (       -- 防分享风险分（share_guard.py）
    key_id INTEGER PRIMARY KEY,
    score REAL NOT NULL DEFAULT 0,
    score_ts REAL NOT NULL DEFAULT 0,
    strikes INTEGER NOT NULL DEFAULT 0,
    paused_until REAL NOT NULL DEFAULT 0,
    warned_ts REAL NOT NULL DEFAULT 0,
    paused_ts REAL NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS upstream_snapshots (   -- 每小时记一次上游真实状态，和我们自己的计数做独立核对
    ts REAL NOT NULL,
    v5_percent REAL,              -- 账号 V5 剩余 %（订阅接口）
    v5_rate REAL,                 -- 实测恢复 %/天（拿不到时为空）
    anlas REAL,                   -- 账号 Anlas 余额
    fresh INTEGER NOT NULL DEFAULT 0   -- 1 = 这次是主动刚读的（不是出图时留下的旧缓存），才能拿来算恢复速度
);
CREATE TABLE IF NOT EXISTS status_hourly (  -- 成员接口每小时各 HTTP 状态码的次数（app/status_stats.py）
    hour INTEGER NOT NULL,
    code INTEGER NOT NULL,
    n INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (hour, code)
);
CREATE TABLE IF NOT EXISTS feedback (      -- 成员用 /反馈 提交的问卷（Discord 弹窗表单，不私信任何人）
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    ts REAL NOT NULL,
    discord_id TEXT NOT NULL,
    username TEXT NOT NULL DEFAULT '',
    key_id INTEGER,
    answers TEXT NOT NULL                  -- JSON：{问题 id: 回答}
);
CREATE INDEX IF NOT EXISTS idx_feedback_ts ON feedback (ts);
CREATE INDEX IF NOT EXISTS idx_feedback_discord_id_ts ON feedback (discord_id, ts DESC);
CREATE TABLE IF NOT EXISTS coupons (       -- 成员的券（现在只有「重置券」：在首页自己点一下，重置当天的 V5 / Anlas 额度）
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    key_id INTEGER NOT NULL,
    kind TEXT NOT NULL DEFAULT 'reset',
    note TEXT NOT NULL DEFAULT '',       -- 发券理由，成员能看到（如「唱得真好听」）
    by TEXT NOT NULL DEFAULT '',
    created_at REAL NOT NULL,
    expires_at REAL NOT NULL,
    used_at REAL
);
CREATE INDEX IF NOT EXISTS idx_coupons_key ON coupons(key_id, used_at);
CREATE TABLE IF NOT EXISTS error_events (
    sig TEXT PRIMARY KEY,          -- 错误特征：异常类型 + 出错位置（或来源 + 去掉数字的消息）
    source TEXT NOT NULL,          -- request / upstream / maintenance:xxx / web:landing / disconnect ...
    level TEXT NOT NULL DEFAULT 'error',
    title TEXT NOT NULL DEFAULT '',
    detail TEXT NOT NULL DEFAULT '',   -- 最近一次的堆栈 / 上下文（只给站长看）
    count INTEGER NOT NULL DEFAULT 0,
    first_seen REAL NOT NULL,
    last_seen REAL NOT NULL,
    last_rid TEXT NOT NULL DEFAULT '',
    last_key INTEGER,
    last_path TEXT NOT NULL DEFAULT '',
    resolved_at REAL               -- 站长标记已处理；之后再出现视为「复发」
);
CREATE INDEX IF NOT EXISTS idx_error_events_resolved_last_seen ON error_events (resolved_at, last_seen DESC);
-- key_idle_reminders 已随「闲置提醒」移除（改为 3 天不生图直接拉黑，不发提醒）；share_confirm 是更早的防分享遗留。
-- 老库在 connect() 里 DROP TABLE IF EXISTS 清理，见下方 waitlist / key_sources 一组。
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
CREATE TABLE IF NOT EXISTS upstream_token_image_concurrency (
    token_id TEXT PRIMARY KEY,
    concurrency INTEGER NOT NULL DEFAULT 1
);
CREATE TABLE IF NOT EXISTS daily_quota_offsets (
    key_id INTEGER NOT NULL,
    day TEXT NOT NULL,
    anlas REAL NOT NULL DEFAULT 0,
    v5 INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (key_id, day)
);
CREATE TABLE IF NOT EXISTS deleted_key_usage_flags (
    key_id INTEGER PRIMARY KEY,
    exclude_global_v5 INTEGER NOT NULL,
    is_test INTEGER NOT NULL DEFAULT 0
);
CREATE TRIGGER IF NOT EXISTS preserve_deleted_key_usage
BEFORE DELETE ON api_keys
BEGIN
    INSERT INTO deleted_key_usage_flags(key_id, exclude_global_v5)
        VALUES (OLD.id, OLD.exclude_global_v5)
        ON CONFLICT(key_id) DO NOTHING;
    DELETE FROM daily_quota_offsets WHERE key_id=OLD.id;
END;
CREATE TRIGGER IF NOT EXISTS prevent_deleted_key_quota_reset
BEFORE INSERT ON daily_quota_offsets
WHEN EXISTS (SELECT 1 FROM deleted_key_usage_flags WHERE key_id=NEW.key_id)
BEGIN
    SELECT RAISE(IGNORE);
END;
