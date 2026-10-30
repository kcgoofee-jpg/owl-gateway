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

test("member generates an image through the gateway", async ({ page, context }) => {
  await asMember(context);
  await page.goto("/workbench");
  await page.locator('textarea[aria-label="提示词"]').fill("an owl, night sky");
  await page.locator(".nai-generate").click();
  await expect(page.locator(".nai-view-img")).toBeVisible({ timeout: 30_000 });
  expect(
    await page.locator(".nai-view-img").evaluate((image) => (image as HTMLImageElement).naturalWidth),
  ).toBeGreaterThan(0);
});

test("V4.5 reference and image-to-image uploads reach the generation request", async ({ page }) => {
  const png = Buffer.from(
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+/qz8AAAAASUVORK5CYII=",
    "base64",
  );
  let generation: Record<string, unknown> | null = null;
  await page.route("**/ai/encode-vibe", (route) =>
    route.fulfill({ status: 200, body: Buffer.from("vibe-encoded-data") }),
  );
  await page.route("**/ai/generate-image", (route) =>
    route.fulfill({ status: 200, contentType: "image/png", body: png }),
  );
  page.on("request", (request) => {
    if (request.url().endsWith("/ai/generate-image")) {
      generation = JSON.parse(request.postData() ?? "{}") as Record<string, unknown>;
    }
  });

  await page.goto("/workbench?preview=1");
  await expect(page.getByRole("button", { name: "V4.5 Full", exact: true })).toBeVisible();
  await page.locator('input[aria-label="选择参考图"]').setInputFiles({
    name: "reference.png",
    mimeType: "image/png",
    buffer: png,
  });
  await page.locator('input[aria-label="选择参考图片"]').setInputFiles({
    name: "initial.png",
    mimeType: "image/png",
    buffer: png,
  });
  await expect(page.locator(".nai-reference-item img")).toBeVisible();
  await expect(page.locator(".nai-i2i-thumb")).toBeVisible();
  await page.locator('textarea[aria-label="提示词"]').fill("portrait, soft light");
  await page.locator(".nai-generate").click();
  await expect(page.locator(".nai-view-img")).toBeVisible();

  const request = generation as {
    action?: string;
    model?: string;
    parameters?: Record<string, unknown>;
  } | null;
  expect(request?.model).toBe("nai-diffusion-4-5-full");
  expect(request?.action).toBe("img2img");
  expect(request?.parameters?.image).toBe(png.toString("base64"));
  expect(request?.parameters?.strength).toBe(0.65);
  expect(request?.parameters?.noise).toBe(0);
  expect(request?.parameters?.reference_image_multiple).toEqual([
    Buffer.from("vibe-encoded-data").toString("base64"),
  ]);
  expect(request?.parameters?.reference_strength_multiple).toEqual([0.6]);
  await expect(page.getByRole("button", { name: "充值" })).toHaveCount(0);
  await page.getByRole("button", { name: "用作初始图" }).click();
  await expect(page.locator(".nai-i2i-pick")).toContainText(/seed-\d+\.png/);
  await page.reload();
  await expect(page.locator(".nai-rail-item img")).toHaveCount(1);
  await expect(page.locator(".nai-view-img")).toBeVisible();
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
