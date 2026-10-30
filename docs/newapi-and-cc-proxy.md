# 同机新增的两个服务：New API 与 Command Code Studio 反代

2026-10-30 上线，都跑在猫头鹰那台机器上，由同一个 Caddy 收口，容器只绑 `127.0.0.1`。

| 域名 | 后端 | 用途 |
|---|---|---|
| `https://newapi.davidzhao.top` | 容器 `new-api` → `127.0.0.1:3300` | New API 大模型中转台（OpenAI 兼容接口） |
| `https://cc.davidzhao.top` | Caddy `reverse_proxy https://commandcode.ai` | Command Code Studio 的反代 |

## New API

```bash
docker run -d --name new-api --restart unless-stopped \
  -p 127.0.0.1:3300:3000 \
  -e TZ=Asia/Shanghai \
  -e SESSION_SECRET=<随机> -e CRYPTO_SECRET=<随机> \
  -v /opt/newapi/data:/data \
  calciumion/new-api:latest
```

- 首次启动是「未初始化」状态，用 `POST /api/setup` 建管理员（当前用户名/密码见服务器 `/opt/newapi/credentials.txt`，权限 600，不进仓库）。
- 数据（SQLite）在 `/opt/newapi/data/one-api.db`，升级先备份这个目录。
- 加渠道：后台「渠道」里填上游 `Base URL` + `Key`；对外提供 `https://newapi.davidzhao.top/v1`（OpenAI 兼容）。
- 网关自己的 `owl-gate` 与本服务无关，互不影响，可单独重启。

## Command Code Studio 反代

Caddy 片段（已写进 `/opt/caddy/Caddyfile`，改完必须用 `/opt/owl/caddy-reload.sh` 加载）：

```caddyfile
cc.davidzhao.top {
	reverse_proxy https://commandcode.ai {
		header_up Host commandcode.ai
		header_up Origin https://commandcode.ai
		header_up Referer https://commandcode.ai/
		header_down Set-Cookie "commandcode.ai" "cc.davidzhao.top"
		header_down Location "https://commandcode.ai" "https://cc.davidzhao.top"
		header_down Location "//commandcode.ai" "//cc.davidzhao.top"
		header_down -Content-Security-Policy
		header_down -X-Frame-Options
		header_down -Report-To
		header_down -NEL
	}
}
```

要点：

- `Host/Origin/Referer` 必须换成官方域名，否则上游的 CSRF 校验和站点路由都会拒；
- 回程把 `Set-Cookie` 里的 `Domain=commandcode.ai` 改成 `cc.davidzhao.top`，否则浏览器直接丢弃 cookie，登录态存不住；
- `Location` 的绝对跳转也要改，不然 302 会把用户送回官网；
- CSP / X-Frame-Options / NEL 删掉，避免和本站不同源引起的拦截与噪音上报。

已知限制：

- **登录要重新做一次**：官网的会话 cookie 绑在 `commandcode.ai` 域上，反代域拿不到，必须在 `cc.davidzhao.top/zh/signin` 重新登录；**用邮箱+密码**，GitHub/Google 的 OAuth 回调固定回官网域名，登录态落不回来。
- 站内绝对链接（例如指向 `https://commandcode.ai/...` 的纯 `<a>`）不会走反代，点到会回到官网。
- 上游走 Cloudflare，机房 IP 目前可用（实测 200/302 正常）；哪天被挑战，需要换出口或用 Cloudflare Worker 转发。

## DNS

两条 A 记录通过 Cloudflare API 建，`proxied=false`（灰云，让 Caddy 直连拿真实客户端 IP）：

```
newapi  A  <OWL_IP>  ttl 120
cc      A  <OWL_IP>  ttl 120
```

## 回滚

```bash
# Caddy 配置
ssh owl-new 'ls /opt/caddy/Caddyfile.bak-*'         # 改动前的备份
ssh owl-new 'cp /opt/caddy/Caddyfile.bak-<时间> /opt/caddy/Caddyfile && /opt/owl/caddy-reload.sh'

# New API
ssh owl-new 'docker rm -f new-api'                   # 数据留在 /opt/newapi/data
```
