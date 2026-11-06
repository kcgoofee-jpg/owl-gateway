#!/usr/bin/env bash
# Close finished requests whose response body never reported completion.
# "committed" means the image was generated and billed; delivery_state=pending means
# the client stopped reading (timeout/disconnect) and the body stream never ended.
# After 10 minutes no live response can still be pending, so mark it interrupted
# instead of leaving the member portal showing "传输中" forever.
set -euo pipefail
DB=${OWL_GATE_DB:-/opt/service-tools/data/nai_gate.db}
now=$(date +%s)
n=$(sqlite3 "$DB" "UPDATE request_settlements SET delivery_state='transport_interrupted', updated_at=$now WHERE delivery_state='pending' AND status='committed' AND updated_at < $now - 600; SELECT changes();")
if [ "${n:-0}" != "0" ]; then
  echo "$(date -Is) closed $n stale pending deliveries"
fi
