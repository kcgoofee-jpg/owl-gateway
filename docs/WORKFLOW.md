# Development and release workflow

One path from edit to production. No pull requests: commit to `main`, CI gates, `tools/deploy.sh` ships.

## 1. Once per clone

```bash
git config core.hooksPath tools/hooks
```

- `commit-msg` rejects CJK text, IPv4 addresses, 17-20 digit IDs and member key refs (`K123`).
- `pre-commit` blocks the production server IP, runs the synthetic-fixture check, and `cargo fmt --check` when Rust files are staged.

## 2. Change

- Work on `main` (or a short-lived local branch merged locally). Keep each commit one logical change.
- Before committing, run what the change touches:
  - Rust: `cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`
  - Web: `cd web && pnpm biome check . && pnpm tsc --noEmit && pnpm vitest run`
  - Fixtures: `tools/ci/check-fixtures.sh` (the repository is public, so fixtures may only hold synthetic data).
  - End to end: start `cargo xtask dev` and `pnpm vite --port 5174`, then `cd web && E2E_BASE=http://localhost:5174 pnpm exec playwright test`. `web/e2e/layout.spec.ts` holds the hard layout rules: at every width no page may end a paragraph with one or two orphan characters, and controls on one row must be equally tall and aligned.
  - Snapshot updates: inputs and current outputs live in each crate's `tests/snapshots/**/*.jsonl`, one case per line. When a behaviour change is intended, run `UPDATE_SNAPSHOT=1 cargo test -p <crate> --test <name>` locally only, read the diff, and say in the commit body which snapshots changed and why (CI forbids updates).
- 更新日志用简单中文，按日期说明改了什么即可，不要求固定分类、版本号或技术细节。
- The 2.x Python code and the byte-for-byte comparison fixtures used during the cutover are kept under the git tags `py-final` and `v3-deviations-final`.

## 3. Commit messages and other public text

The repository is public. Commit messages, docs and comments use clear technical language. 更新日志使用简单中文描述功能变化。

Never write any of the following into the repository or a commit message:

- member names, handles, Discord IDs or key numbers;
- counts of members or requests, incident narratives, or which member uses which client;
- server addresses, community or guild names and IDs, owner decisions with dates.

Background, data and before/after tables go to the owner's report, not into git.

Format: `type(scope): imperative summary`, for example `fix(policy): name the legacy reference field in the error`.

User-facing strings (errors, UI, bot replies) stay in Chinese: members read them.

## 4. CI (push to `main`)

`web`, `rust`, `e2e`, `secrets`, `deploy-safety`, `image` and `nai-ops` run in parallel; `e2e` waits for `web`. The E2E layout checks use a lightweight Chinese font instead of downloading the 61 MB Noto CJK package. Apt setup, browser installation and Playwright each have explicit timeouts so a stalled dependency cannot hold a run indefinitely. Artifacts are built alongside the checks, so a red run still uploads them, but a red run is never deployable.

## 5. Deploy

```bash
tools/deploy.sh --wait     # wait for CI on main HEAD, then deploy it
tools/deploy.sh            # deploy the latest green run
tools/deploy.sh <run-id>   # deploy a specific green run
```

The script refuses runs that did not fully succeed and skips if that build is already live. It downloads and stages the candidate image without changing `current` or `previous`, then promotes the candidate and restarts immediately — no queue wait (owner 10-27); the 1-2 minute downtime window is covered by the Caddy maintenance page, and the old gateway finishes accepted work while shutting down. After restart it checks `healthcheck`, `nai-ops smoke` and the public `/healthz`.

`tools/deploy.sh --wait` reports CI state and its GitHub run URL every minute, and stops waiting after one hour by default (`OWL_CI_WAIT_TIMEOUT_SECONDS` changes that limit). Queue waiting defaults to 1800 seconds with 5-second polling; set `OWL_QUEUE_WAIT_TIMEOUT_SECONDS` and `OWL_QUEUE_POLL_SECONDS` in the local deployment environment file to change these values.

Rollback: `ssh` in and run `/opt/owl/cutover.sh previous`.

Changes that need follow-up on the host (cron, scripts under `deploy/v3/ops/`) are listed in the commit body and applied by hand after the deploy.

## 6. Watch

`tools/watch-prod.py` reads the usage log every two minutes and prints one line per finding, nothing when healthy:

- infra: restarts, `/healthz`, bot container, memory, disk, new error events;
- upstream: any 401 / 403 / 429, bursts of 5xx or connection drops, a disabled account;
- repeat: one key failing the same way three times in an hour (usually a parameter worth rejecting up front);
- new: a rejection or error text not seen in the previous seven days, and reset messages that say 明天 before 07:00;
- storm: one key rejected 30 times in 30 minutes for reasons other than a site-wide stop;
- capacity: site daily total or V5 exhausted (with the time), an account at its daily cap, queue timeouts, long waits, no successful image for 15 minutes;
- plan: no V5 plan after 07:15, site-paid V5 started.

By default it exits after the first batch of findings, so a background job wakes whoever is watching; `--forever` keeps running under a line-based monitor. `--replay START END` re-runs the log checks over a past window, which is how the thresholds were tuned.

## 7. Disk

Every worktree has its own `target/`. `tools/clean-targets.sh` drops caches of idle worktrees (`--days 0` drops all, `--dry-run` reports).
