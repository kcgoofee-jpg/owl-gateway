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
    discord_id TEXT,
    features TEXT,
    quota_limits TEXT,
    v5_pinned INTEGER,
    disabled_reason TEXT,
    last_image_at REAL   -- 最后一次成功生图（含放大 / 导演 / Vibe）：闲置回收只看它，查额度不算活跃（2026-10-13 站长）
);
