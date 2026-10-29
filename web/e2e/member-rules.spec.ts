import { type Cookie, expect, test } from "@playwright/test";
import { readCreds } from "./devEnv";
import {
  adminSession,
  misalignedControls,
  noHorizontalScroll,
  seriousViolations,
  watchConsole,
} from "./helpers";
import { orphans } from "./orphans";

let session: Cookie[] = [];
// These viewport cases edit the same synthetic database settings; do not race their save/restore steps.
test.describe.configure({ mode: "serial" });

test.beforeAll(async ({ browser, baseURL }) => {
  const password = readCreds().admin_password ?? process.env.E2E_ADMIN_PASSWORD;
  if (!password) throw new Error("member-rules requires a running synthetic dev stack");
  session = await adminSession(browser, baseURL, password);
});
for (const width of [1440, 390, 320]) {
  test(`成员规则与例外 ${width}px`, async ({ page, context }, info) => {
    await context.addCookies(session);
    await page.setViewportSize({ width, height: 900 });
    const errors = watchConsole(page, "/admin/api/events");
    await page.goto("/admin/onboarding");
    await expect(page.getByRole("heading", { name: "默认规则", exact: true })).toBeVisible();
    await expect(page.getByRole("button", { name: "添加身份组" })).toBeVisible();
    await page.getByRole("button", { name: "添加身份组" }).click();
    await page.getByRole("button", { name: "添加身份组" }).click();
    expect(await noHorizontalScroll(page, { loose: true })).toBe(true);
    expect(await orphans(page)).toEqual([]);
    expect(await misalignedControls(page)).toEqual([]);
    expect(await seriousViolations(page)).toEqual([]);
    await page.screenshot({ path: info.outputPath(`member-rules-${width}.png`), fullPage: true });
    await page.goto("/admin/limits");
    await expect(page.getByRole("heading", { name: "全站预算（0 停用）", exact: true })).toBeVisible();
    await expect(page.getByRole("heading", { name: "账号保护与节奏", exact: true })).toBeVisible();
    expect(await noHorizontalScroll(page, { loose: true })).toBe(true);
    expect(await orphans(page)).toEqual([]);
    expect(await misalignedControls(page)).toEqual([]);
    await page.screenshot({ path: info.outputPath(`runtime-protection-${width}.png`), fullPage: true });
    await page.goto("/admin/members");
    await page
      .getByRole("button", { name: /^展开.*的详情$/ })
      .first()
      .click();
    await expect(page.getByRole("heading", { name: "有效规则" })).toBeVisible();
    expect(await misalignedControls(page)).toEqual([]);
    await page.getByRole("button", { name: "成员例外", exact: true }).click();
    const dialog = page.getByRole("dialog");
    await expect(dialog).toBeVisible();
    await expect(dialog.getByText("额度来源", { exact: true })).toHaveCount(0);
    await expect(dialog.getByText("身份组", { exact: true })).toHaveCount(0);
    const field = dialog.getByLabel("V4.5 免费图/天", { exact: true });
    const original = await field.inputValue();
    await field.fill(original === "0" ? "1" : "0");
    await expect(dialog.getByRole("button", { name: "保存", exact: true })).toBeEnabled();
    await dialog.getByRole("button", { name: "恢复V4.5 免费图/天默认", exact: true }).click();
    const defaults = (await (await page.request.get("/admin/api/member-rules")).json()).defaults;
    await expect(field).toHaveValue(String(defaults.daily_images));
    if (String(defaults.daily_images) === original)
      await expect(dialog.getByRole("button", { name: "保存", exact: true })).toBeDisabled();
    else await expect(dialog.getByRole("button", { name: "保存", exact: true })).toBeEnabled();
    expect(await noHorizontalScroll(page, { loose: true })).toBe(true);
    expect(await orphans(page)).toEqual([]);
    expect(await misalignedControls(page)).toEqual([]);
    expect(await seriousViolations(page)).toEqual([]);
    await page.screenshot({ path: info.outputPath(`member-exceptions-${width}.png`), fullPage: true });
    expect(errors).toEqual([]);
  });
}

test("默认规则、成员例外和清空例外真实保存", async ({ page, context }) => {
  await context.addCookies(session);
  const original = await page.request.get("/admin/api/member-rules");
  const baseline = (await original.json()).defaults;
  let memberPath = "";
  let originalOverrides: unknown;
  try {
    await page.goto("/admin/onboarding");
    const defaults = page.getByRole("region", { name: "默认规则", exact: true });
    const changed = baseline.daily_images === 123 ? 124 : 123;
    await defaults.getByLabel("V4.5 免费图/天", { exact: true }).fill(String(changed));
    const saved = page.waitForResponse(
      (r) => r.url().endsWith("/admin/api/member-rules") && r.request().method() === "PUT",
    );
    await defaults.getByRole("button", { name: "保存", exact: true }).click();
    expect((await saved).status()).toBe(200);
    await page.reload();
    await expect(defaults.getByLabel("V4.5 免费图/天", { exact: true })).toHaveValue(String(changed));
    await page.goto("/admin/members");
    const row = page.getByRole("button", { name: /^展开.*的详情$/ }).first();
    const keyId = Number((await row.getAttribute("aria-controls"))?.replace("mb-d-", ""));
    const members = (await (await page.request.get("/admin/api/members")).json()).members;
    const target = members.find((member: { id: number }) => member.id === keyId);
    expect(target.discord_id).toBeTruthy();
    memberPath = `/admin/api/members/${target.discord_id}/rules`;
    originalOverrides = (await (await page.request.get(memberPath)).json()).overrides;
    await row.click();
    await page.getByRole("button", { name: "成员例外", exact: true }).click();
    const dialog = page.getByRole("dialog");
    await dialog.getByLabel("V4.5 免费图/天", { exact: true }).fill("0");
    const overridden = page.waitForResponse(
      (r) => new URL(r.url()).pathname === memberPath && r.request().method() === "PUT",
    );
    await dialog.getByRole("button", { name: "保存", exact: true }).click();
    expect((await overridden).status()).toBe(200);
    const persisted = await (await page.request.get(memberPath)).json();
    expect(persisted.effective.daily_images).toBe(0);
    expect(persisted.overrides.daily_images).toBe(0);
    await expect(dialog).toBeHidden();
    await page.getByRole("button", { name: "成员例外", exact: true }).click();
    await dialog.getByRole("button", { name: "恢复V4.5 免费图/天默认", exact: true }).click();
    const cleared = page.waitForResponse(
      (r) => new URL(r.url()).pathname === memberPath && r.request().method() === "PUT",
    );
    await dialog.getByRole("button", { name: "保存", exact: true }).click();
    expect((await cleared).status()).toBe(200);
    const inherited = await (await page.request.get(memberPath)).json();
    expect(inherited.effective.daily_images).toBe(changed);
    expect(inherited.overrides.daily_images).toBeNull();
  } finally {
    if (memberPath && originalOverrides) await page.request.put(memberPath, { data: originalOverrides });
    await page.request.put("/admin/api/member-rules", { data: baseline });
  }
});
