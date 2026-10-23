#!/usr/bin/env python3
"""Production watch driven by the usage log, not just liveness.

Polls every INTERVAL seconds over one ssh call, keeps a little state between polls and prints
one line per finding. Healthy polls print nothing.

    tools/watch-prod.py                      run until the first finding batch, print it, exit 0
    tools/watch-prod.py --forever            keep running (for a line-based monitor)
    tools/watch-prod.py --replay START END   replay a past window (unix seconds) poll by poll, log checks only

Findings, each reported once per cooldown or once per service day:
  infra     server unreachable, gateway restarted, /healthz not 200, bot down, memory, disk
  upstream  401 / 403 / 429 from upstream (any), 5xx bursts, connection drops
  repeat    one key failing the same way again and again (deterministic failure worth a 400 up front)
  new       a rejection or error text not seen in the previous 7 days (new code path or wrong copy);
            a reset message saying 明天 between midnight and the reset
  storm     one key hammering the gateway while being rejected (client retry loop)
  capacity  site daily total exhausted, site V5 exhausted, an account at its daily cap, queue timeouts,
            long queue waits, no successful image for 15 minutes outside quiet hours
  plan      no V5 plan for the service day after 07:15; site-paid V5 started

Needs ~/.config/owl/owl.env (OWL_IP, never printed) and ~/.ssh/naigate_ed25519.
STATE (default ~/.cache/owl-watch.json) keeps dedupe state across runs.
"""
import json
import os
import re
import subprocess
import sys
import time
from datetime import datetime, timedelta, timezone

INTERVAL = int(os.environ.get("INTERVAL", "120"))
STATE = os.path.expanduser(os.environ.get("STATE", "~/.cache/owl-watch.json"))
MEM_WARN_KB = int(os.environ.get("MEM_WARN_KB", "460000"))
BJ = timezone(timedelta(hours=8))
RESET_HOUR, QUIET = 7, (3, 7)

# The remote half: one read-only pass over the database and container state, printed as JSON.
REMOTE = r'''
import json, sqlite3, subprocess, sys, time
since, until, infra = float(sys.argv[1]), float(sys.argv[2]), sys.argv[3] == "1"
db = sqlite3.connect("file:/opt/service-tools/data/nai_gate.db?mode=ro", uri=True, timeout=10)
q = lambda sql, *a: db.execute(sql, a).fetchall()
out = {"until": until}
out["rows"] = q("""SELECT ts, key_id, kind, model, status, reason, up_status, token_id, detail, client
                   FROM usage_log WHERE ts > ? AND ts <= ? AND status != 'ok' ORDER BY ts""", since, until)
out["seen7d"] = None
if sys.argv[4] == "1":
    out["seen7d"] = [r[0] for r in q("""SELECT DISTINCT detail FROM usage_log
                     WHERE ts > ? AND ts <= ? AND status != 'ok'""", since - 7 * 86400, since)]
w = until - 900
out["ok15"] = q("""SELECT count(*), avg(wait_ms), max(wait_ms), avg(dur_ms) FROM usage_log
                   WHERE ts > ? AND ts <= ? AND status = 'ok' AND kind LIKE 'image%'""", w, until)[0]
out["req15"] = q("SELECT count(*) FROM usage_log WHERE ts > ? AND ts <= ? AND kind LIKE 'image%'", w, until)[0][0]
waits = [r[0] for r in q("""SELECT wait_ms FROM usage_log WHERE ts > ? AND ts <= ? AND status = 'ok'
                             AND kind LIKE 'image%' ORDER BY wait_ms""", w, until)]
out["p90wait"] = waits[int(len(waits) * 0.9)] if len(waits) >= 10 else None
out["paid"] = q("""SELECT count(*) FROM usage_log WHERE ts > ? AND ts <= ? AND detail LIKE '%站付V5%'""",
                since, until)[0][0]
setting = lambda k: (q("SELECT value FROM site_settings WHERE key = ?", k) or [[None]])[0][0]
out["caps"] = {"default": setting("guard_account_daily_cap"), "tokens": setting("guard_token_daily_caps")}
out["counters"] = q("SELECT token_id, day, images, v5 FROM upstream_token_counters WHERE day >= date(?, 'unixepoch', '-1 day')", until)
out["disabled"] = [r[0] for r in q("SELECT token_id FROM upstream_token_enabled WHERE enabled = 0")]
out["errev"] = q("""SELECT source, substr(title, 1, 90), count FROM error_events WHERE last_seen > ? AND last_seen <= ?
                    AND source NOT IN ('disconnect') AND source NOT LIKE 'web:%'""", since, until)
if infra:
    sh = lambda c: subprocess.run(c, shell=True, capture_output=True, text=True).stdout.strip()
    pid = sh("docker inspect -f '{{.State.Pid}}' owl-gate")
    out["infra"] = {
        "started": sh("docker inspect -f '{{.State.StartedAt}}' owl-gate")[:19],
        "health": sh("docker inspect -f '{{.State.Health.Status}}' owl-gate"),
        "bot": sh("docker inspect -f '{{.State.Running}}' owl-naimei"),
        "hz": sh("curl -s -m 5 -o /dev/null -w '%{http_code}' http://127.0.0.1:3003/healthz"),
        "rss": sh(f"awk '/RssAnon/{{print $2}}' /proc/{pid}/status") if pid else "",
        "disk": sh("df -P / | awk 'NR==2{print $5}'").rstrip("%"),
    }
print(json.dumps(out, ensure_ascii=False))
'''


def remote(since, until, infra, want_seen):
    env = dict(os.environ)
    for line in open(os.path.expanduser("~/.config/owl/owl.env")):
        m = re.match(r"\s*(?:export\s+)?(\w+)=(.*)", line)
        if m:
            env[m.group(1)] = m.group(2).strip().strip("'\"")
    cmd = ["ssh", "-i", os.path.expanduser("~/.ssh/naigate_ed25519"), "-o", "IdentitiesOnly=yes",
           "-o", "BatchMode=yes", "-o", "LogLevel=QUIET", "-o", "ConnectTimeout=15", f"root@{env['OWL_IP']}",
           f"python3 - {since} {until} {1 if infra else 0} {1 if want_seen else 0}"]
    r = subprocess.run(cmd, input=REMOTE, capture_output=True, text=True, timeout=120)
    if r.returncode != 0 or not r.stdout.strip():
        raise RuntimeError(re.sub(r"\d{1,3}(\.\d{1,3}){3}", "x.x.x.x", (r.stderr or "no output")[:160]))
    return json.loads(r.stdout)


def sig(detail):
    """Normalise a detail so counts, sizes and times do not make every line look new."""
    m = re.match(r"upstream (\d+)（([\w.-]+)", detail or "")
    if m:  # upstream errors carry the request shape; group them by status and model only
        return f"upstream {m.group(1)} {m.group(2)}"
    s = re.sub(r"\d+(\.\d+)?", "#", detail or "")
    return re.sub(r"\s+", " ", s).strip()[:120]


def service_day(ts):
    return (datetime.fromtimestamp(ts, BJ) - timedelta(hours=RESET_HOUR)).strftime("%Y-%m-%d")


def hm(ts):
    return datetime.fromtimestamp(ts, BJ).strftime("%m-%d %H:%M")


class Watch:
    def __init__(self, state):
        self.s = state
        self.s.setdefault("once", {})  # finding key -> expiry ts
        self.s.setdefault("seen", None)  # known detail signatures
        self.recent = []  # non-ok rows of the last hour

    def once(self, key, now, ttl):
        if self.s["once"].get(key, 0) > now:
            return False
        self.s["once"][key] = now + ttl
        return True

    def day_once(self, key, now):
        return self.once(f"{service_day(now)}:{key}", now, 86400)

    def poll(self, d, now, infra=True):
        out = []
        say = lambda kind, text: out.append(f"{hm(now)} [{kind}] {text}")
        if self.s["seen"] is None and d.get("seen7d") is not None:
            self.s["seen"] = sorted({sig(x) for x in d["seen7d"]})
        seen = set(self.s["seen"] or [])
        rows = [dict(zip(("ts", "key", "kind", "model", "status", "reason", "up", "token", "detail", "client"), r))
                for r in d["rows"]]
        self.recent = [r for r in self.recent if r["ts"] > now - 3600] + rows
        last10 = [r for r in self.recent if r["ts"] > now - 600]
        last30 = [r for r in self.recent if r["ts"] > now - 1800]

        # upstream: auth / rate problems are never normal; 5xx and connection drops in bursts
        for code in (401, 403, 429):
            n = sum(1 for r in rows if r["up"] == code)
            if n and self.once(f"up{code}", now, 1800):
                toks = sorted({(r["token"] or "?")[-6:] for r in rows if r["up"] == code})
                say("upstream", f"upstream {code} x{n} on account(s) {', '.join(toks)}")
        five = [r for r in last10 if (r["up"] or 0) >= 500 or "Internal Server Error" in (r["detail"] or "")]
        if len(five) >= 3 and self.once("5xx", now, 1800):
            keys = sorted({r["key"] for r in five})
            say("upstream", f"{len(five)} upstream 5xx in 10 min across {len(keys)} key(s); models "
                f"{sorted({r['model'] for r in five})}")
        drops = [r for r in last10 if r["status"] == "error" and not r["up"]]
        if len(drops) >= 5 and self.once("drops", now, 1800):
            say("upstream", f"{len(drops)} connection drops / timeouts to upstream in 10 min")

        # repeat: the same key failing the same way is usually a parameter the gateway could reject up front
        groups = {}
        for r in self.recent:
            if r["status"] == "error":
                groups.setdefault((r["key"], sig(r["detail"])), []).append(r)
        for (key, sg), rs in groups.items():
            if len(rs) >= 3 and self.day_once(f"repeat:{key}:{sg}", now):
                say("repeat", f"key {key} failed {len(rs)}x in 1h with the same error ({rs[-1]['model']}, "
                    f"client {(rs[-1]['client'] or '')[:30]}): {rs[-1]['detail'][:110]}")

        # new: a message nobody has seen in a week
        for r in rows:
            sg = sig(r["detail"])
            if sg and sg not in seen:
                seen.add(sg)
                say("new", f"first time in 7 days ({r['status']}, {r['reason'] or '-'}, key {r['key']}): "
                    f"{r['detail'][:130]}")
        self.s["seen"] = sorted(seen)

        # storm: one key keeps sending while rejected
        per_key = {}
        for r in last30:
            # retries against a site-wide stop are expected and reported once as capacity
            if r["status"] == "rejected" and r["reason"] not in ("daily_cap", "quiet_cap", "site_quota"):
                per_key.setdefault(r["key"], []).append(r)
        for key, rs in per_key.items():
            if len(rs) >= 30 and self.day_once(f"storm:{key}", now):
                reasons = {}
                for r in rs:
                    reasons[r["reason"] or "-"] = reasons.get(r["reason"] or "-", 0) + 1
                say("storm", f"key {key} rejected {len(rs)}x in 30 min {reasons} (client {(rs[-1]['client'] or '')[:30]})")

        # wording: before the 07:00 reset the next reset is today, never tomorrow
        early = [r for r in rows if "明天" in (r["detail"] or "")
                 and datetime.fromtimestamp(r["ts"], BJ).hour < RESET_HOUR]
        if early and self.day_once(f"tomorrow_before_reset:{early[0]['reason']}", now):
            say("new", f"message says 明天 before the {RESET_HOUR}:00 reset (key {early[0]['key']}): "
                f"{early[0]['detail'][:110]}")

        # capacity
        for reason, label in (("daily_cap", "site daily image total exhausted"),
                              ("site_quota", "site quota exhausted"),
                              ("breaker", "breaker tripped: generation paused")):
            hit = [r for r in rows if r["reason"] == reason]
            if hit and self.day_once(f"cap:{reason}:{sig(hit[0]['detail'])[:40]}", now):
                say("capacity", f"{label} at {hm(hit[0]['ts'])}: {hit[0]['detail'][:100]}")
        qt = [r for r in last10 if r["reason"] == "queue_timeout"]
        if len(qt) >= 3 and self.once("queue_timeout", now, 1800):
            say("capacity", f"{len(qt)} queue timeouts (90 s) in 10 min")
        n_ok, avg_wait, max_wait, avg_dur = d["ok15"]
        p90 = d.get("p90wait")
        if p90 and p90 > 75000 and self.once("wait", now, 3600):
            say("capacity", f"queue wait p90 {p90 / 1000:.0f}s over 15 min ({n_ok} images, avg {avg_wait / 1000:.0f}s)")
        hour = datetime.fromtimestamp(now, BJ).hour
        quiet_near = any(QUIET[0] <= datetime.fromtimestamp(t, BJ).hour < QUIET[1] for t in (now, now - 900))
        capped = any(r["reason"] == "daily_cap" for r in last30)
        if not quiet_near and not capped and n_ok == 0 and d["req15"] >= 5 and self.once("no_ok", now, 1800):
            say("capacity", f"no successful image in 15 min while {d['req15']} requests arrived")
        caps = d["caps"]
        default = int(float(caps["default"] or 0))
        tok_caps = json.loads(caps["tokens"] or "{}")
        today = service_day(now)
        for tok, day, images, v5 in d["counters"]:
            cap = default
            tc = tok_caps.get(tok)
            if tc and float(tc.get("until", 0)) > now:
                cap = int(tc["cap"])
            if day == today and cap and images >= cap and self.day_once(f"acct:{tok}", now):
                say("capacity", f"account ..{tok[-6:]} reached its daily cap {cap} ({images} images, {v5} V5)")
        for tok in d["disabled"]:
            if self.day_once(f"disabled:{tok}", now):
                say("upstream", f"account ..{tok[-6:]} is disabled")

        if d["paid"] and self.day_once("paid", now):
            say("plan", f"site-paid V5 started ({d['paid']} so far this poll)")
        for src, title, cnt in d["errev"]:
            if self.once(f"err:{src}:{title}", now, 3600):
                say("infra", f"error event {src}: {title} (x{cnt})")

        if infra and "infra" in d:
            i = d["infra"]
            if self.s.get("started") and i["started"] != self.s["started"]:
                say("infra", f"gateway restarted ({self.s['started']} -> {i['started']})")
            self.s["started"] = i["started"]
            if i["hz"] != "200":
                say("infra", f"/healthz returned {i['hz'] or 'nothing'} (health {i['health']})")
            if i["bot"] != "true":
                say("infra", "bot container is not running")
            if i["rss"].isdigit() and int(i["rss"]) > MEM_WARN_KB and self.once("mem", now, 3600):
                say("infra", f"gateway RssAnon {int(i['rss']) // 1024} MB")
            if i["disk"].isdigit() and int(i["disk"]) >= 85 and self.once("disk", now, 21600):
                say("infra", f"disk {i['disk']}% used")
        return out


def load():
    try:
        return json.load(open(STATE))
    except (OSError, ValueError):
        return {}


def save(s):
    os.makedirs(os.path.dirname(STATE), exist_ok=True)
    s["once"] = {k: v for k, v in s.get("once", {}).items() if v > time.time() - 86400}
    json.dump(s, open(STATE, "w"))


def replay(start, end):
    w = Watch({})
    t = start
    first = True
    while t < end:
        nxt = min(t + INTERVAL, end)
        d = remote(t, nxt, False, first)
        first = False
        for line in w.poll(d, nxt, infra=False):
            print(line, flush=True)
        t = nxt


def main():
    if len(sys.argv) == 4 and sys.argv[1] == "--replay":
        return replay(float(sys.argv[2]), float(sys.argv[3]))
    forever = "--forever" in sys.argv
    state = load()
    w = Watch(state)
    since = max(state.get("since", 0), time.time() - 3 * INTERVAL)
    while True:
        now = time.time()
        try:
            d = remote(since, now, True, state.get("seen") is None)
            lines = w.poll(d, now)
            since = now
        except Exception as e:  # noqa: BLE001 - any failure to look is itself a finding
            lines = [f"{hm(now)} [infra] cannot read production: {e}"] if w.once("unreachable", now, 900) else []
        state["since"] = since
        save(state)
        for line in lines:
            print(line, flush=True)
        if lines and not forever:
            return
        time.sleep(INTERVAL)


if __name__ == "__main__":
    main()
