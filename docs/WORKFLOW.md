# Development and release workflow

One path from edit to production. No pull requests: commit to `main`, CI gates, `tools/deploy.sh` ships.

## 1. Once per clone

```bash
git config core.hooksPath tools/hooks
```

- `commit-msg` rejects CJK text, IPv4 addresses, 17-20 digit IDs and member key refs (`K123`).
- `pre-commit` blocks the production server IP, runs the synthetic-fixture check, and `cargo fmt --check` when Rust files are staged.

## 2. Change

- Keep each commit one logical change.
- Run checks for the affected code locally; CI owns the complete Rust and web unit suites:
  - Rust: `cargo fmt --all --check`, check the affected crates, then run their relevant tests.
  - Web: `cd web && pnpm check && pnpm test`.
  - Release smoke: start `cargo xtask dev` and Vite, then run `cd web && pnpm exec playwright test e2e/release.spec.ts`.
  - Full browser regression: run `pnpm exec playwright test` locally or dispatch the **UI regression** workflow. Layout, animation, accessibility and extended browser scenarios are optional regression checks, outside the release gate.
- Release smoke requires synthetic member and admin credentials from `cargo xtask dev`; missing credentials fail the run rather than silently skipping it.
- Snapshot updates: run `UPDATE_SNAPSHOT=1 cargo test -p <crate> --test <name>` locally only, inspect the diff and explain intended behavior changes. CI forbids snapshot updates.
- 更新日志用简单中文，按日期说明改了什么即可。

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

The required run contains web lint/types/unit tests, Rust formatting/Clippy/tests, four browser release-smoke scenarios, fixture/secret checks, deployment guards, and release artifacts. The production web bundle is built in the image job; the web check job does not build it again. Image smoke verifies the actual container and embedded landing bundle.

Full UI regression runs only on manual dispatch through `ui-regression.yml`. Its result does not block ordinary releases. Keep quota, accounting, request recovery, migration and scheduling regression tests in the required Rust suite.

Artifact builds run alongside checks. `tools/deploy.sh` accepts only a successful CI run for the selected commit; a failed run is never deployable. Reuse that run for deployment instead of rerunning the full suite.

## 5. Deploy

```bash
tools/deploy.sh --wait     # wait for CI on main HEAD, then deploy it
tools/deploy.sh --wait --no-notify  # same deployment without a completion webhook
tools/deploy.sh            # deploy the latest green run
tools/deploy.sh <run-id>   # deploy a specific green run
```

The script refuses runs that did not fully succeed and skips if that build is already live. It downloads and stages the candidate image without changing `current` or `previous`, then promotes the candidate and restarts immediately — no queue wait (owner 10-27); the 1-2 minute downtime window is covered by the Caddy maintenance page, and the old gateway finishes accepted work while shutting down. After restart it checks `healthcheck`, `nai-ops smoke` and the public `/healthz`.

`tools/deploy.sh --wait` reports CI state and its GitHub run URL every minute, and stops waiting after one hour by default (`OWL_CI_WAIT_TIMEOUT_SECONDS` changes that limit). The cutover stops the old gateway and waits for its accepted requests to finish before starting the new process.

Rollback: `ssh` in and run `/opt/owl/cutover.sh previous`.

Changes that need follow-up on the host (cron, scripts under `deploy/v3/ops/`) are listed in the commit body and applied by hand after the deploy.

## 6. Watch

`tools/watch-prod.py` reads the usage log every two minutes and prints one line per finding, nothing when healthy:

- infra: restarts, `/healthz`, bot container, memory, disk;
- upstream: any 401 / 403 / 429, bursts of 5xx or connection drops, a disabled account;
- repeat: one key failing the same way three times in an hour (usually a parameter worth rejecting up front);
- new: a rejection or error text not seen in the previous seven days, and reset messages that say 明天 before 07:00;
- storm: one key rejected 30 times in 30 minutes for reasons other than a site-wide stop;
- capacity: site daily total or V5 exhausted (with the time), an account at its daily cap, queue timeouts, long waits, no successful image for 15 minutes;
- plan: site-paid V5 started.

By default it exits after the first batch of findings, so a background job wakes whoever is watching; `--forever` keeps running under a line-based monitor. `--replay START END` re-runs the log checks over a past window, which is how the thresholds were tuned.

## 7. Disk

Every worktree has its own `target/`. `tools/clean-targets.sh` drops caches of idle worktrees (`--days 0` drops all, `--dry-run` reports).
