# zcode-api for 奶妹

TriDefender/zcode-api on the same host as the gateway. Loopback only.

1. Copy this directory to `/opt/zcode-proxy` (mode 700).
2. Put `ZCODE_PROXY_CREDENTIAL_SECRET` and `ZCODE_PROXY_API_KEY` in `/opt/zcode-proxy/.env` (mode 600). Use the same API key as `NAIMEI_LLM_API_KEY`.
3. Login once (any browser): `docker run --rm --user bun -e ZCODE_PROXY_CREDENTIAL_SECRET -e ZCODE_PROXY_CONFIG=/data/config.yaml -v /opt/zcode-proxy:/data -v /opt/zcode-proxy/home:/home/bun/.zcode-proxy ghcr.io/tridefender/zcode-proxy:latest bun index.js auth login zai`
4. Copy the resulting `credentials.json` next to compose, then `docker compose -f /opt/zcode-proxy/compose.yml up -d`.
5. Smoke: `curl -sS http://127.0.0.1:8090/v1/chat/completions` with `glm-5.3-flash`.
6. Join `owl_internal`. Point 奶妹 at `http://zcode-proxy:8080` / `glm-5.3-flash`. Recreate only `owl-naimei`.

This account uses start-plan (no paid coding-plan). Do not publish 8090.
