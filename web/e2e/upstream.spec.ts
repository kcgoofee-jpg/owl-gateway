import { expect, test } from "@playwright/test";
import { readCreds } from "./devEnv";
import { misalignedControls, noHorizontalScroll, seriousViolations, watchConsole } from "./helpers";
import { orphans } from "./orphans";

const now = 1_793_000_000;
test.beforeEach(async ({ context, page, baseURL }) => {
  const cookie = readCreds().admin;
  test.skip(!cookie, "需要本地演示会话");
  await context.addCookies([
    { name: "nai_gate_admin", value: cookie ?? "", url: baseURL ?? "http://localhost:5174" },
  ]);
  await page.route("**/admin/api/upstream", async (route) => {
    const response = await route.fetch();
    const body = await response.json();
    const original = body.accounts[0];
    body.accounts = [1, 2].map((position) => ({
      ...original,
      position,
      token_id: `synthetic-${position}`,
      token: `pst-sy…demo${position}`,
      images_today: 123,
      v5_used: 42,
      v5_daily_limit: 300,
      image_active: 0,
      image_concurrency: 1,
      load_share_24h: 50,
      balance_share: 50,
      v5: {
        account: position,
        percent: 0,
        purchased_percent: 0,
        is_negative: false,
        active: true,
        tier: 3,
        expires_at: now + 864000,
        is_grace_period: false,
        account_type: 0,
        fixed_anlas: 0,
        purchased_anlas: 500,
        checked_at: now,
        low: true,
        stale: false,
        uncertain: false,
        error: null,
        time_until_next_percent_seconds: 3660,
        current_response_recharge_per_day_estimate: 5.5,
        historical_sample_based_recharge_per_day_estimate: 5.2,
      },
      info: {
        account: position,
        token_id: `synthetic-${position}`,
        ban_status: "not_banned",
        ban_message: null,
        email_verified: false,
        account_created_at: now - 864000,
        trial_activated: false,
        trial_actions_left: 0,
        trial_images_left: 0,
        last_success: now,
        last_error: null,
        last_error_at: null,
        stale: false,
      },
      balance: {
        balance: 450,
        fixed: 0,
        purchased: 450,
        expires_at: now + 864000,
        days_left: 10,
        avg_daily: 20,
        projection: position === 1 ? "waste" : "runs_out",
        projection_value: position === 1 ? 150 : 2.5,
        snapshot_age_days: 1,
        stale: false,
        approximate: false,
        cycle_rolled: false,
        unattributed_anlas: 0,
      },
      w1h: {
        ok: 12,
        error: 1,
        rejected: 2,
        up_401: 0,
        up_403: 0,
        up_429: 1,
        timing_samples: 12,
        avg_wait_ms: 0,
        p90_wait_ms: 400,
        avg_dur_ms: 2000,
      },
      w24h: {
        ok: 123,
        error: 1,
        rejected: 2,
        up_401: 0,
        up_403: 0,
        up_429: 1,
        timing_samples: 123,
        avg_wait_ms: 100,
        p90_wait_ms: 900,
        avg_dur_ms: 3000,
      },
    }));
    await route.fulfill({ response, json: body });
  });
});

for (const width of [1440, 390, 320]) {
  test(`上游完整读数 ${width}px：零值、两类预测和移动布局`, async ({ page }, info) => {
    await page.setViewportSize({ width, height: 1000 });
    await page.goto("/admin/upstream");
    const cards = page.locator(".up-account");
    await expect(cards).toHaveCount(2);
    const first = cards.first();
    const value = (label: string) =>
      first
        .locator(".up-facts > div")
        .filter({ has: page.locator("dt").filter({ hasText: new RegExp(`^${label}$`) }) })
        .locator("dd");
    await expect(value("账号类型")).toHaveText("0");
    await expect(value("试用操作")).toHaveText("0");
    await expect(value("试用图片")).toHaveText("0");
    await expect(value("邮箱验证")).toHaveText("未验证");
    await expect(value("宽限期")).toHaveText("否");
    await expect(value("下次恢复")).toHaveText("1 小时 1 分钟");
    await expect(value("当前推算恢复")).toHaveText("5.5% / 天");
    await expect(value("历史推算恢复")).toHaveText("5.2% / 天");
    await expect(value("额外购买电量")).toHaveText("0%");
    await expect(value("到期预测")).toHaveText("到期剩余约 150 固定 Anlas");
    await expect(cards.last()).toContainText("约 2.5 天用完固定额度");
    await expect(first.getByRole("row", { name: "平均排队 0 秒 0.1 秒" })).toBeVisible();
    await expect(first.getByRole("heading", { name: "订阅与账号" })).toBeVisible();
    await expect(page.getByText(/官方读数.*来自两把/)).toHaveCount(0);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await page.screenshot({ path: info.outputPath(`upstream-${width}.png`), fullPage: true });
  });
}

test("上游空读数：缺失不显示成零", async ({ page }) => {
  await page.unroute("**/admin/api/upstream");
  await page.route("**/admin/api/upstream", async (route) => {
    const response = await route.fetch();
    const body = await response.json();
    body.accounts = body.accounts
      .slice(0, 1)
      .map((a: object) => ({ ...a, v5: null, info: null, balance: null }));
    await route.fulfill({ response, json: body });
  });
  await page.goto("/admin/upstream");
  const card = page.locator(".up-account");
  await expect(card.locator(".up-highlights strong").first()).toHaveText("—");
  await expect(card.locator(".up-highlights strong").nth(1)).toHaveText("—");
  await expect(card.getByText("未核对", { exact: true }).first()).toBeVisible();
});

for (const width of [1440, 390, 320]) {
  test(`账号完整信息 ${width}px：按需查询、完整字段和关闭清除`, async ({ page }, info) => {
    const calls: string[] = [];
    await page.route("**/admin/api/upstream/*/details", async (route) => {
      const id = new URL(route.request().url()).pathname.split("/").at(-2) ?? "";
      calls.push(id);
      expect(route.request().method()).toBe("POST");
      expect(route.request().headers()["x-nai-admin-csrf"]).toBeTruthy();
      await route.fulfill({
        headers: { "cache-control": "no-store" },
        json: {
          token_id: id,
          queried_at: now,
          retry_after: 60,
          information: {
            status: 200,
            error: null,
            data: {
              plainTextEmail: `private-canary-${id}@example.invalid`,
              accountCreatedAt: 1700000000,
              emailVerified: false,
              trialImagesLeft: 0,
              futureField: { amount: 0, enabled: false },
            },
          },
          subscription: {
            status: 200,
            error: null,
            data: { tier: 3, paymentProcessorData: { receipt: "private-payment-canary" }, unknownCredits: 0 },
          },
          priority: {
            status: 200,
            error: null,
            data: { taskPriority: 0, maxPriorityActions: 1000, nextRefillAt: 0 },
          },
        },
      });
    });
    const errors = watchConsole(page, "/admin/api/events");
    await page.setViewportSize({ width, height: 900 });
    await page.goto("/admin/upstream");
    const buttons = page.getByRole("button", { name: "完整信息", exact: true });
    await expect(buttons).toHaveCount(2);
    expect(calls).toEqual([]);
    await buttons.first().click();
    const dialog = page.getByRole("dialog", { name: "账号 1 · 完整信息" });
    await expect(
      dialog.getByText("private-canary-synthetic-1@example.invalid", { exact: true }),
    ).toBeVisible();
    await expect(dialog.getByText("futureField", { exact: true })).toBeVisible();
    await expect(dialog.getByText(/2023/)).toBeVisible();
    await expect(dialog.getByText("amount", { exact: true })).toBeVisible();
    await expect(dialog.getByRole("button", { name: /秒后刷新/ })).toBeDisabled();
    await dialog.getByRole("button", { name: "订阅", exact: true }).click();
    await expect(dialog.getByText("private-payment-canary", { exact: true })).toBeVisible();
    await dialog.getByRole("button", { name: "优先级", exact: true }).click();
    await expect(dialog.getByText("任务优先级", { exact: true })).toBeVisible();
    expect(await noHorizontalScroll(page, { loose: true })).toBe(true);
    expect(await misalignedControls(page)).toEqual([]);
    expect(await orphans(page)).toEqual([]);
    expect(await seriousViolations(page)).toEqual([]);
    await page.screenshot({ path: info.outputPath(`account-details-${width}.png`), fullPage: true });
    await dialog.getByRole("button", { name: "关闭", exact: true }).last().click();
    await expect(page.getByText(/private-canary|private-payment-canary/)).toHaveCount(0);
    await buttons.nth(1).click();
    const second = page.getByRole("dialog", { name: "账号 2 · 完整信息" });
    await expect(
      second.getByText("private-canary-synthetic-2@example.invalid", { exact: true }),
    ).toBeVisible();
    await expect(page.getByText("private-canary-synthetic-1@example.invalid", { exact: true })).toHaveCount(
      0,
    );
    await second.getByRole("button", { name: "关闭", exact: true }).last().click();
    expect(calls).toEqual(["synthetic-1", "synthetic-2"]);
    expect(await page.evaluate(() => JSON.stringify({ ...localStorage, ...sessionStorage }))).not.toMatch(
      /private-canary|private-payment-canary/,
    );
    if (width === 1440) {
      await page.getByRole("button", { name: "退出", exact: true }).click();
      await expect(page.getByLabel("后台密码")).toBeVisible();
      await expect(page.getByText(/private-canary|private-payment-canary/)).toHaveCount(0);
    }
    expect(errors).toEqual([]);
  });
}
