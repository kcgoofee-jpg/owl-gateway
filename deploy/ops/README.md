# 运维脚本

3.0 的运维命令都在 `nai-ops`（Rust，见 `crates/gate-ops`：`patrol` / `set` / `get` / `report` / `traffic` / `snapshot` / `ro-sanitize` / `smoke`），
服务器上的定时脚本和 cron 在 `deploy/v3/ops/`（`crontab` 写明了线上的三条）。
部署、切换、回滚、巡检、备份、常用命令和安全红线都只在 [docs/OPERATIONS.md](../../docs/OPERATIONS.md) 里写一份，这里不重复。本目录放的是：

| 文件 | 在哪跑 | 用途 |
|---|---|---|
| `migrate_restore.sh` | 新服务器 | 灾难恢复：从 S3 拉最新加密备份 → 解密 → 恢复库、`.env`、3.0 部署文件与 Caddy 配置 → 用 CI 镜像启动 |
| `pull-ro-snapshot.sh` | 本机 | 拉每天的脱敏只读快照（`/opt/backups/ro/`）到本地，排查问题用 |
| `pull-backup-mac.sh` | 本机 | 把加密备份多拉一份到本机（异地第三份） |
| `systemd/nai-patrol.{service,timer}` | 服务器 | 每 5 分钟跑一次 `/opt/owl/nai-ops patrol --oneline --alert`（安装方法见 OPERATIONS「巡检」） |
| `../Caddyfile.access-log.example` | 服务器 | Caddy JSON 访问日志片段（不含 IP / 请求头），给 `nai-ops traffic` 用 |
| `../risk-terms.example.txt` | — | `nai-ops report` 内容风险词表的空示例（真实词表只放服务器） |
| `SYSTEM-FACTS.md` | — | 系统事实速查（数字以代码为准） |
