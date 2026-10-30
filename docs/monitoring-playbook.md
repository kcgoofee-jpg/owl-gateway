# 巡检手册（生产环境，只读）

给下一次值班/自查用。目标：**15 分钟内判断站点是否健康**，并能带走可复现的证据。

## 环境

- SSH：`ssh owl-new`（`~/.ssh/config` 里已有别名；该配置带 LocalForward 3000，会打印 `bind [127.0.0.1]:3000: Address already in use`，忽略即可）。
- 容器：`owl-gate`（网关，宿主 3003 → 容器 8000）、`owl-naimei`（Discord 机器人）、`caddy`（host 网络）、`new-api`（中转台，127.0.0.1:3300）。
- 数据库：宿主 `/opt/service-tools/data/nai_gate.db`（SQLite，只读查询）。
- 后台接口：`https://gate.davidzhao.top/admin/api/*`，密码在 `/opt/service-tools/.env` 的 `ADMIN_PASSWORD`。
  **登录频率**：每 IP 5 次 / 300 秒，超过 429。巡检请**复用 cookie jar**，不要每轮重登。

## 采集项（每轮 ~60 秒）

```bash
A=https://gate.davidzhao.top; JAR=/var/tmp/monitor.jar
# 1. 可用性
for p in /healthz /public/live /; do curl -s -o /dev/null -w "$p %{http_code}\n" "$A$p"; done
# 2. 错误日志（去重取代表行）
docker logs owl-gate --since 90s 2>&1 | grep -iE 'error|warn|panic|database is locked' | tail -8
docker logs owl-naimei --since 90s 2>&1 | tail -5
# 3. 资源
docker stats --no-stream --format '{{.Name}} {{.CPUPerc}} {{.MemUsage}}' owl-gate owl-naimei caddy
# 4. 请求结果分布（列名见下）
sqlite3 /opt/service-tools/data/nai_gate.db \
  "select status, coalesce(reason,'') r, count(*) n, avg(wait_ms)/1000 avg_wait_s from usage_log where ts > strftime('%s','now')-300 group by 1,2 order by n desc;"
# 5. 排队与上游
curl -s "$A/queue-status"
docker exec owl-gate nai-ops smoke http://127.0.0.1:8000 | tail -3    # 每 3 轮跑一次即可
```

`usage_log` 列：`id ts key_id key_name kind model status images anlas tokens unconfirmed_anlas detail wait_ms dur_ms client up_status rid src ver reason token_id xrw lang chua hfp bid origin`。

## 判定阈值（触发即告警）

| 现象 | 阈值 | 含义 |
|---|---|---|
| `/healthz` 或 `/public/live` | 连续 2 轮非 200 | 网关或反代挂了 |
| 容器重启 | StartedAt 变新 | 崩溃/被 OOM/被部署 |
| 日志 | `panic`、`database is locked`、大量 5xx | 代码或存储问题 |
| `usage_log` | 5 分钟内 ≥5 条非 ok，或出现新 `reason` | 上游/额度/排队异常 |
| 内存 | 容器超上限 85% | 泄漏或流量突增 |
| `queue_full` 占比 | > 30% 且持续 | 容量瓶颈（不是故障） |
| `upstream 5xx` | 连续出现 | NovelAI 侧故障，网关会如实转成 5xx 并说明未扣费 |

## 已知噪音（不要当故障）

- `client_disconnected` / `stream_disconnected_before_dispatch`：用户在排队时关页面，未扣费，正常。
- `rejected / queue_full`：全站队列按 90 秒能出完的量动态算，满了就快速拒绝，属设计行为。
- `WARN allow: V5 low allowance account=token-xxx`：唯一上游的 V5 额度偏低，产品决定不补账号，记一笔即可。
- 后台 `login 429`：登录限流（5 次/300 秒），换 cookie 复用即可。

## 历史坑

- **2026-10-30**：`cutover.sh` 的实测子进程在父脚本被中断后变成孤儿，每 0.5 秒打 `/healthz` 和 `/`，两天把 Caddy 访问日志刷到 10 分钟一轮转，巡检时误判成"外部探针太多"。已修：父进程没了子进程自己退 + `EXIT/INT/TERM` kill。**日志被刷爆时先 `ps -eo pid,etime,args | grep connect-to` 查孤儿。**
- **2026-10-23**：`nai-ops get|set` 容器内默认连 `127.0.0.1:3003`（那是宿主端口），容器里应连 8000；现在 compose 里已设 `ADMIN_API_BASE`。
