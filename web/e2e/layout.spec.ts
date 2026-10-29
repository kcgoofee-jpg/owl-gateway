/**
 * 排版硬规则（站长 10-15）：任何宽度下都不能出现「最后一行只剩一两个字」，同一行的控件必须等高、上下对齐。
 * 首页（游客）和后台各页，在常见的桌面 / 平板 / 手机宽度下逐一检查。
 *
 * 运行：E2E_BASE=http://localhost:5174 pnpm exec playwright test e2e/layout.spec.ts
 */
import { type Cookie, expect, type Page, test } from "@playwright/test";
import { readCreds } from "./devEnv";
import { adminSession, asMember, misalignedControls } from "./helpers";
import { orphans } from "./orphans";

const WIDTHS = [1440, 1280, 1024, 900, 768, 430, 390, 375];

/** 视觉上同一行、相邻（间距 < 48px）的控件：高度和顶边都要一致（误差 1.5px）。 */
async function check(page: Page, where: string) {
  const o = await orphans(page);
  const m = await misalignedControls(page);
  expect.soft(o, `${where}：最后一行只剩一两个字`).toEqual([]);
  expect.soft(m, `${where}：同一行控件不齐`).toEqual([]);
}

test("首页（游客）：各宽度无孤字、控件对齐", async ({ page }) => {
  for (const w of WIDTHS) {
    await page.setViewportSize({ width: w, height: 900 });
    await page.goto("/");
    await expect(page.locator(".lp-status__text")).not.toBeEmpty();
    await check(page, `首页 ${w}px`);
  }
});

test.describe("首页（成员）", () => {
  const member = readCreds().member;
  test.skip(!member, "没有成员会话（先跑 cargo xtask dev）");
  test("各宽度无孤字、控件对齐", async ({ page, context }) => {
    await asMember(context);
    for (const w of WIDTHS) {
      await page.setViewportSize({ width: w, height: 900 });
      await page.goto("/");
      await expect(page.locator(".lp-quota")).toBeVisible();
      await expect(page.locator(".lp-status__text")).not.toBeEmpty();
      await check(page, `首页（成员） ${w}px`);
    }
  });
});

const ADMIN = [
  "/admin",
  "/admin/members",
  "/admin/logs/usage",
  "/admin/logs/actions",
  "/admin/upstream",
  "/admin/onboarding",
  "/admin/limits",
  "/admin/system",
];

test.describe("后台", () => {
  let session: Cookie[] = [];
  test.beforeAll(async ({ browser, baseURL }) => {
    const password = process.env.E2E_ADMIN_PASSWORD ?? readCreds().admin_password;
    test.skip(!password, "没有后台密码（先跑 cargo xtask dev）");
    session = await adminSession(browser, baseURL, password ?? "");
  });

  // 成员页的筛选横栏：桌面 1280 / 手机 375 下必须是单行（标签同顶同高），并且展开被拒原因后也没有孤字、控件对齐
  for (const w of [1280, 375]) {
    test(`成员筛选横栏 ${w}px：单行、等高、无孤字`, async ({ page, context }) => {
      await context.addCookies(session);
      await page.setViewportSize({ width: w, height: 900 });
      await page.goto("/admin/members");
      await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
      await page.waitForLoadState("networkidle");
      const bar = page.getByRole("group", { name: "筛选成员" });
      await bar.getByRole("button", { name: /^24 小时被拒过/ }).click();
      const tops = await bar.evaluate((el) => [
        ...new Set([...el.querySelectorAll("button")].map((x) => Math.round(x.getBoundingClientRect().top))),
      ]);
      expect(tops, "筛选标签必须在同一行").toHaveLength(1);
      await check(page, `/admin/members 筛选横栏 ${w}px`);
    });
  }

  for (const w of [1440, 1024, 390]) {
    test(`各页 ${w}px：无孤字、控件对齐`, async ({ page, context }) => {
      await context.addCookies(session);
      await page.setViewportSize({ width: w, height: 900 });
      for (const path of ADMIN) {
        await page.goto(path);
        await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
        await page.waitForLoadState("networkidle");
        await check(page, `${path} ${w}px`);
      }
    });
  }
});
