/**
 * 生图工作台端到端：成员走网关真实出图（xtask dev 的假上游），
 * 多张批量、token 计数、窄屏布局探针；游客能看到编辑器外壳但不出图。
 * 成员会话来自 cookies.json，没有就整组跳过。
 */
import { expect, type Page, test } from "@playwright/test";
import { readCreds } from "./devEnv";
import { asMember, misalignedControls, noHorizontalScroll, seriousViolations, watchConsole } from "./helpers";
import { orphans } from "./orphans";

const hasMember = Boolean(readCreds().member);

/** 各宽度的布局探针（无横向滚动 + 孤字 + 控件对齐）。 */
async function checkLayout(page: Page, where: string) {
  const o = await orphans(page);
  expect(o, `${where} 孤字: ${o.join(" | ")}`).toEqual([]);
  const m = await misalignedControls(page);
  expect(m, `${where} 控件不齐: ${m.join(" | ")}`).toEqual([]);
  expect(await noHorizontalScroll(page)).toBe(true);
}

test.describe("生图工作台", () => {
  test.skip(!hasMember, "需要成员会话（cargo xtask dev 的 cookies.json）");
  // 真实出图走同一个假上游，串行避免并发限流
  test.describe.configure({ mode: "serial" });

  test("首页入口跳转 → 工作台真实出图一张并进历史", async ({ page, context }) => {
    await asMember(context);
    const errors = watchConsole(page);
    await page.goto("/");
    const entry = page.locator("#workbench-entry");
    await expect(entry).toBeVisible();
    await entry.click();
    await expect(page).toHaveURL(/\/workbench$/);
    await expect(page.locator(".nai-generate")).toBeVisible();
    await page.locator('textarea[aria-label="提示词"]').fill("1girl, seaside, sunset");
    // 假上游回装着真 PNG 的 zip：走完网关排队 / 记账，图进历史栏并进入选中态
    await page.locator(".nai-generate").click();
    await expect(page.locator(".nai-rail-item img")).toBeVisible({ timeout: 30_000 });
    await expect(page.locator(".nai-rail-item img")).toHaveCount(1);
    await expect(page.locator(".nai-view-img")).toBeVisible();
    // 出过图后额度条刷新
    await expect(page.locator(".nai-side-foot .nai-quota")).toBeVisible();
    expect(errors).toEqual([]);
    expect(await seriousViolations(page)).toEqual([]);
    await checkLayout(page, "工作台出图页");
  });

  test("多张出图：图片数 2 → 历史两张", async ({ page, context }) => {
    await asMember(context);
    const errors = watchConsole(page);
    await page.goto("/workbench");
    await expect(page.locator(".nai-generate")).toBeVisible();
    await page.locator(".nai-seg--count button", { hasText: "2" }).click();
    await page.locator('textarea[aria-label="提示词"]').fill("1boy, mountain, dawn");
    await page.locator(".nai-generate").click();
    await expect(page.locator(".nai-rail-item img")).toHaveCount(2, { timeout: 45_000 });
    // 生成按钮文案随图片数变化
    await expect(page.locator(".nai-generate")).toContainText("Generate 2 Images");
    expect(errors).toEqual([]);
    expect(await seriousViolations(page)).toEqual([]);
  });

  test("提示词 token 计数：输入后进度条有宽度", async ({ page, context }) => {
    await asMember(context);
    await page.goto("/workbench");
    await expect(page.locator(".nai-generate")).toBeVisible();
    const bar = page.locator(".nai-tokenbar i");
    await expect(bar).toHaveCSS("width", "0px");
    await page.locator('textarea[aria-label="提示词"]').fill("1girl, silver hair, café window");
    await expect(bar).not.toHaveCSS("width", "0px");
  });

  test("窄屏布局：375 / 320 无横向滚动", async ({ page, context }) => {
    await asMember(context);
    await page.goto("/workbench");
    await expect(page.locator(".nai-generate")).toBeVisible();
    for (const w of [375, 320]) {
      await page.setViewportSize({ width: w, height: 900 });
      await expect(page.locator(".nai-generate")).toBeVisible();
      await checkLayout(page, `工作台 ${w}px`);
    }
  });

  test("游客访问 /workbench：见编辑器外壳，生成给登录提示", async ({ page }) => {
    const errors = watchConsole(page);
    await page.goto("/workbench");
    await expect(page.locator(".nai-frame")).toBeVisible();
    await expect(page.locator(".nai-generate")).toBeVisible();
    await page.locator(".nai-generate").click();
    await expect(page.locator(".toast").first()).toBeVisible();
    expect(errors).toEqual([]);
  });
});
