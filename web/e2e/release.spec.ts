/** Release checks use the real local gateway, synthetic credentials and fake upstream. */
import { expect, test } from "@playwright/test";
import { readCreds } from "./devEnv";
import { adminSession, asMember } from "./helpers";

test.beforeAll(() => {
  const creds = readCreds();
  expect(creds.member, "Start cargo xtask dev and set E2E_COOKIES").toBeTruthy();
  expect(creds.admin_password).toBeTruthy();
});

test("guest landing renders and member APIs require credentials", async ({ page, request }) => {
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.goto("/");
  await expect(page.locator(".lp-status__text")).not.toBeEmpty();
  expect((await request.get("/v1/me")).status()).toBe(401);
  expect(errors).toEqual([]);
});

// 镜像页由网关直接渲染（静态资源在 STATIC_DIR，不进仓库）；CI 里没有镜像时跳过。
const GATE = process.env.E2E_GATE ?? "http://127.0.0.1:18000";

test("member entry lands on the mirrored image page", async ({ page, context, request }) => {
  const mirror = await request.get(`${GATE}/image`);
  test.skip(mirror.status() !== 200, "镜像资源未就绪（需要 STATIC_DIR 指向 mirror/）");
  await asMember(context, GATE);
  await page.goto(`${GATE}/workbench`);
  await expect(page).toHaveURL(/\/image$/);
  // 官网镜像页的最小信号：模型行 + 生成按钮
  await expect(page.getByText("Model", { exact: true })).toBeVisible({ timeout: 20_000 });
  await expect(page.locator("button", { hasText: /Generate \d+ Image/ }).first()).toBeVisible();
});

test("admin login loads member management", async ({ page, context, browser, baseURL }) => {
  const cookies = await adminSession(browser, baseURL, readCreds().admin_password ?? "");
  await context.addCookies(cookies);
  await page.goto("/admin/members");
  await expect(page.getByRole("heading", { level: 1, name: "成员" })).toBeVisible();
  await expect(page.getByRole("button", { name: /展开.*的详情/ }).first()).toBeVisible();
});

test("retired status routes are absent and health remains available", async ({ request }) => {
  for (const path of ["/status", "/status.json", "/_owl/status-page.css", "/_owl/status-page.js"]) {
    expect((await request.get(path)).status(), path).toBe(404);
  }
  for (const path of ["/healthz", "/readyz", "/livez"]) {
    const response = await request.get(path);
    expect(response.status(), path).toBe(200);
    expect((await response.json()).ok).toBe(true);
  }
});
