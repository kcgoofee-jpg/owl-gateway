#!/usr/bin/env bash
# 离线结构升级：把库改到 gate-db 当前的 SCHEMA_VERSION。
#
# 只在网关停机时跑（部署脚本的 cutover 窗口里），启动时不做在线迁移——
# bootstrap 看到版本不对就直接报错退出，所以顺序必须是：停网关 → 跑本脚本 → 起网关。
# 不传参数时默认库是 $DATA_DIR/nai_gate.db；也可以在参数里指定路径。
#
# 每加一版结构，就在 MIGRATIONS 里补一个 `N:函数名` 并写出 N→N+1 的那一段。
set -euo pipefail

DB=${1:-/opt/service-tools/data/nai_gate.db}
[ -f "$DB" ] || { echo "找不到库：$DB" >&2; exit 1; }

q() { sqlite3 "$DB" "$1"; }

# v2 → v3：usage_log 去掉 6 个客户端提示列（xrw / lang / chua / hfp / bid / origin，只写不读）
# 和早已不在 schema 里的 ver 列。整表重建，索引按 schema.sql 重新建。
v2_to_v3() {
    if [ "$(q "SELECT count(*) FROM pragma_table_info('usage_log') WHERE name='xrw'")" = "0" ] \
        && [ "$(q "SELECT count(*) FROM pragma_table_info('usage_log') WHERE name='ver'")" = "0" ]; then
        echo "  usage_log 已是目标形态，跳过"
        return 0
    fi
    q "BEGIN;
       CREATE TABLE usage_log_new (
           id INTEGER PRIMARY KEY AUTOINCREMENT,
           ts REAL NOT NULL,
           key_id INTEGER,
           key_name TEXT NOT NULL DEFAULT '',
           kind TEXT NOT NULL,
           model TEXT NOT NULL DEFAULT '',
           status TEXT NOT NULL,
           images INTEGER NOT NULL DEFAULT 0,
           anlas REAL NOT NULL DEFAULT 0,
           tokens INTEGER NOT NULL DEFAULT 0,
           unconfirmed_anlas REAL NOT NULL DEFAULT 0,
           detail TEXT NOT NULL DEFAULT '',
           wait_ms INTEGER NOT NULL DEFAULT 0,
           dur_ms INTEGER NOT NULL DEFAULT 0,
           client TEXT NOT NULL DEFAULT '',
           up_status INTEGER NOT NULL DEFAULT 0,
           rid TEXT NOT NULL DEFAULT '',
           src TEXT NOT NULL DEFAULT '',
           reason TEXT NOT NULL DEFAULT '',
           token_id TEXT
       );
       INSERT INTO usage_log_new
           (id, ts, key_id, key_name, kind, model, status, images, anlas, tokens, unconfirmed_anlas,
            detail, wait_ms, dur_ms, client, up_status, rid, src, reason, token_id)
       SELECT id, ts, key_id, key_name, kind, model, status, images, anlas, tokens, unconfirmed_anlas,
              detail, wait_ms, dur_ms, client, up_status, rid, src, reason, token_id
       FROM usage_log;
       DROP TABLE usage_log;
       ALTER TABLE usage_log_new RENAME TO usage_log;
       CREATE INDEX IF NOT EXISTS idx_log_ts ON usage_log (ts DESC);
       CREATE INDEX IF NOT EXISTS idx_log_key ON usage_log (key_id, ts DESC);
       CREATE INDEX IF NOT EXISTS idx_log_ts_upstatus ON usage_log (ts, up_status);
       CREATE INDEX IF NOT EXISTS idx_log_key_id ON usage_log (key_id, id DESC);
       CREATE INDEX IF NOT EXISTS idx_log_rid ON usage_log (rid);
       CREATE INDEX IF NOT EXISTS idx_log_src ON usage_log (src);
       CREATE INDEX IF NOT EXISTS idx_log_image_stats ON usage_log (status, kind, key_id, ts, images);
       COMMIT;"
    echo "  usage_log 重建完成：$(q "SELECT count(*) FROM usage_log") 行，$(q "SELECT count(*) FROM pragma_table_info('usage_log')") 列"
}

# v3 → v4：删掉三张只写不读的表。
# - discord_user_guilds：成员所在 Discord 服务器列表，没有任何查询读过它，占了全库 38%；
# - deleted_key_owners：为已删除的周报留的「Key → Discord 账号」；
# - ref_images：成员参考图的 sha256 / dHash，读它的是已删除的关联扫描。
v3_to_v4() {
    local gone=0
    for t in discord_user_guilds deleted_key_owners ref_images; do
        if [ "$(q "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='$t'")" = "1" ]; then
            echo "  删表 $t（$(q "SELECT count(*) FROM $t") 行，$(q "SELECT COALESCE(SUM(pgsize),0)/1024 FROM dbstat WHERE name LIKE '${t}%' OR name LIKE 'idx_${t}%'") KB）"
            q "DROP TABLE $t;"
            gone=$((gone + 1))
        fi
    done
    echo "  删掉 $gone 张表"
}

# v4 → v5：存图频道的分享记录（谁、哪条消息、何时）。不存正文和图片。
v4_to_v5() {
    q "CREATE TABLE IF NOT EXISTS share_posts (
           message_id TEXT PRIMARY KEY,
           discord_id TEXT NOT NULL,
           channel_id TEXT NOT NULL,
           created_at REAL NOT NULL
       );
       CREATE INDEX IF NOT EXISTS idx_share_posts_member ON share_posts (discord_id, created_at);"
    echo "  share_posts 已就绪"
}

MIGRATIONS=(v2_to_v3 v3_to_v4 v4_to_v5)

before=$(q "PRAGMA user_version")
echo "库：$DB"
echo "当前结构版本：$before"
while :; do
    v=$(q "PRAGMA user_version")
    if [ "$v" -ge 5 ]; then
        break
    fi
    fn="v${v}_to_v$((v + 1))"
    found=""
    for m in "${MIGRATIONS[@]}"; do [ "$m" = "$fn" ] && found=1; done
    [ -n "$found" ] || { echo "没有 ${v} → $((v + 1)) 的迁移，停下" >&2; exit 1; }
    echo "执行 $fn"
    "$fn"
    q "PRAGMA user_version = $((v + 1))"
done
q "PRAGMA wal_checkpoint(TRUNCATE)" >/dev/null || true
q "VACUUM"
after=$(q "PRAGMA user_version")
echo "完成：结构版本 $after，库大小 $(du -h "$DB" | cut -f1)"
[ "$before" = "$after" ] && echo "（本来就是这个版本，没有改动结构）"
exit 0
