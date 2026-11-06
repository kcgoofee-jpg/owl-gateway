#!/usr/bin/env bash
# 一次性数据处置：把旧日志归档到服务器本地，然后从库里删掉；顺手清掉已下线功能留下的表与设置，
# 并把库升到当前结构版本（user_version = 2）。
#
#   deploy/ops/purge-legacy-logs.sh            # 归档 + 删除 + VACUUM，不启动网关
#   ARCHIVE=/some/where purge-legacy-logs.sh   # 换归档目录
#
# 顺序很重要：这个脚本跑完之后库是「新形态」，旧版网关读不了（它的 bootstrap 会再补一堆列并吐快照），
# 所以**跑完必须紧接着 `tools/deploy.sh` 推新镜像**，中间不要再 `docker compose up`。
#
# 归档内容（默认 /opt/backups/legacy-logs-YYYYMMDD/，权限 700）：
#   nai_gate.db.full.gz     删之前的整库快照（最后兜底）
#   usage_log.jsonl.gz      请求日志
#   usage_request_links.jsonl.gz
#   admin_actions.jsonl.gz  操作日志
#   abuse_signals.jsonl.gz  旧风控待审信号
#   anlas_reconciliations.jsonl.gz
#   MANIFEST.txt            行数、时间范围、sha256
set -euo pipefail

DATA=${OWL_DATA_DIR:-/opt/service-tools/data}
DB=$DATA/nai_gate.db
STAMP=$(date +%Y%m%d)
ARCHIVE=${ARCHIVE:-/opt/backups/legacy-logs-$STAMP}
GATE=${OWL_GATE:-owl-gate}
COMPOSE=(docker compose -p "${OWL_PROJECT:-owl}" --env-file /opt/service-tools/.env -f /opt/owl/compose.yml)

log() { printf '[%(%F %T)T] %s\n' -1 "$*"; }
die() { log "✗ $*"; exit 1; }

[[ -f $DB ]] || die "找不到 $DB"
[[ -x /opt/owl/nai-ops ]] || die "找不到 /opt/owl/nai-ops（先跑 cutover.sh install-ops）"

# 今天是服务日：当天的日志留着，成员打开「我的请求」不会突然变空
TODAY_START=$(sqlite3 "$DB" "SELECT strftime('%s', date('now', 'localtime'));")
[[ $TODAY_START =~ ^[0-9]+$ ]] || die "算不出今天的起点"

df -Pk "$DATA" | awk 'NR==2 {exit !($4 > 300*1024)}' || die "磁盘剩余不足 300MB，先清理"

log "① 停网关（Caddy 会显示维护页）"
"${COMPOSE[@]}" stop -t 120 gate || true

mkdir -p "$ARCHIVE"
chmod 700 "$ARCHIVE"

log "② 整库快照 → $ARCHIVE/nai_gate.db.full.gz"
/opt/owl/nai-ops snapshot "$ARCHIVE/nai_gate.db.full.gz"

dump_table() {
  local table=$1
  local exists
  exists=$(sqlite3 "$DB" "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='$table';")
  [[ $exists == 1 ]] || { log "   （$table 不存在，跳过）"; return 0; }
  local rows
  rows=$(sqlite3 "$DB" "SELECT count(*) FROM $table;")
  log "③ 导出 $table（$rows 行）"
  sqlite3 "$DB" "SELECT * FROM $table;" -json | gzip -9 >"$ARCHIVE/$table.jsonl.gz"
}

for t in usage_log usage_request_links admin_actions abuse_signals anlas_reconciliations; do
  dump_table "$t"
done

log "④ 写 MANIFEST 并校验归档能读回"
{
  echo "owl 旧日志归档 $STAMP"
  echo "源库：$DB"
  for f in "$ARCHIVE"/*.gz; do
    echo "$(basename "$f")  $(du -h "$f" | cut -f1)  sha256=$(shasum -a 256 "$f" | cut -d' ' -f1)"
  done
  echo
  echo "行数（删之前）："
  for t in usage_log usage_request_links admin_actions abuse_signals anlas_reconciliations; do
    n=$(sqlite3 "$DB" "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='$t';")
    if [[ $n == 1 ]]; then
      echo "  $t  $(sqlite3 "$DB" "SELECT count(*) FROM $t;")"
    fi
  done
  echo
  echo "删除口径：ts < $TODAY_START（今天 0 点之前）"
} >"$ARCHIVE/MANIFEST.txt"

for t in usage_log usage_request_links admin_actions; do
  want=$(sqlite3 "$DB" "SELECT count(*) FROM $t;")
  got=$(gzip -dc "$ARCHIVE/$t.jsonl.gz" | grep -c '^' || true)
  [[ $want == "$got" ]] || die "归档校验失败：$t 库里 $want 行，归档读回 $got 行。什么都没删，可以安全放弃"
done
log "   归档校验通过"

log "⑤ 删除今天 0 点之前的日志"
sqlite3 "$DB" "DELETE FROM usage_log WHERE ts < $TODAY_START;"
sqlite3 "$DB" "DELETE FROM usage_request_links WHERE usage_log_id NOT IN (SELECT id FROM usage_log);"
sqlite3 "$DB" "DELETE FROM admin_actions WHERE ts < $TODAY_START;"

log "⑥ 删已下线功能的表与设置"
sqlite3 "$DB" "DROP TABLE IF EXISTS abuse_signals;"
sqlite3 "$DB" "DROP TABLE IF EXISTS anlas_reconciliations;"
sqlite3 "$DB" <<'SQL'
-- 只删「代码里已经 0 引用」的设置键：已下线功能留下的残留状态。
-- 判断方法：逐键 grep 源码（含动态拼接的 runtime_* 要排除），确认无人再读。
DELETE FROM site_settings WHERE key IN (
  -- 日志保留 / Anlas 核对 / 全站功能开关
  'usage_origin_keep_days','anlas_reconciliation_retry_at','features_global',
  'sitepaid_hint_dismissed',
  -- 自动驾驶（已删；熔断的 autopilot_breaker_until 是新代码在用，不在此列）
  'autopilot_breaker','autopilot_economy','autopilot_economy_at','autopilot_history',
  'autopilot_key_guard','autopilot_last','autopilot_slots','kernel_last','economy_mode',
  -- 额度算法实验（影子对比 / 动态额度）
  'quota_shadow_history','quota_shadow_state','quota_algo_history','quota_algo_last',
  'quota_algo_day','quota_base_now','quota_ceiling','quota_ceiling_max','quota_step',
  'quota_v5_day_plan',
  -- Anlas 自动分配（anlas_auto 早下线）
  'anlas_member_daily_cap','anlas_min_key_age_days',
  -- 防分享打分 / 网页登录暂停开关 / 一次性确认
  'share_guard_mode','web_login_paused','global_daily_anlas_until',
  'ack_registration_ready','ack_v5_oversell',
  -- 成员规则迁移标记（迁移代码已整体删除，标记只剩占位）
  'member_policy_v1_migrated'
);
SQL

log "⑦ 收尾死状态：pending / uncertain 结算"
sqlite3 "$DB" "UPDATE request_settlements SET delivery_state='transport_interrupted', updated_at=strftime('%s','now') WHERE delivery_state='pending' AND status!='committed';"

log "⑧ VACUUM 并把结构版本盖成 2"
before=$(du -m "$DB" | cut -f1)
sqlite3 "$DB" "VACUUM;"
sqlite3 "$DB" "PRAGMA user_version = 2;"
after=$(du -m "$DB" | cut -f1)
log "   库 $before MB → $after MB"

log "⑨ 删掉 2.x 老快照（3.0 的 bootstrap 已经打不开它）"
rm -f /opt/backups/weekly/nai_gate-2026-10-25.db.gz

log "✓ 完成。下一步：tools/deploy.sh 推新镜像（现在网关是停的，别再 compose up 起旧版）"
