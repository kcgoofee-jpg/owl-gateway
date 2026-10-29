/**
 * 生图工作台（Beta）端到端：成员从首页入口跳到 /workbench，走完整出图与批量链路（xtask dev 的假上游），
 * 再做窄屏布局探针；游客只看到登录引导。成员会话来自 cookies.json，没有就整组跳过。
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

test.describe("生图工作台（Beta）", () => {
  test.skip(!hasMember, "需要成员会话（cargo xtask dev 的 cookies.json）");

  test("首页入口跳转 → 工作台真实出图一张并进画廊", async ({ page, context }) => {
    await asMember(context);
    const errors = watchConsole(page);
    await page.goto("/");
    const entry = page.locator("#workbench-entry");
    await expect(entry).toBeVisible();
    await expect(entry).toHaveText("生图工作台");
    await entry.click();
    await expect(page).toHaveURL(/\/workbench$/);
    await expect(page.locator(".nai-generate")).toBeVisible();

    const draw = page.locator('section[aria-label="出图"]');
    await draw.locator(".wb-side textarea").first().fill("1girl, seaside, sunset");
    await page.getByRole("button", { name: /生成/ }).click();
    // 假上游回装着真 PNG 的 zip：走完网关排队 / 记账，图出现在舞台与画廊
    await expect(page.locator(".wb-shot img")).toBeVisible({ timeout: 30_000 });
    await expect(page.locator(".wb-thumb")).toHaveCount(1);

    // 大图弹窗有种子与提示词，可下载
    await page.locator(".wb-shot").click();
    const dialog = page.locator("dialog[open]");
    await expect(dialog).toContainText("种子");
    await expect(dialog).toContainText("提示词");
    await expect(dialog).toContainText("下载");
    await dialog.locator("footer").getByRole("button", { name: "关闭" }).click();
    await expect(dialog).toBeHidden();

    // 出过图后额度行还在（reloadMe 刷新过）
    await expect(page.locator(".wb-quota")).toBeVisible();

    expect(errors).toEqual([]);
    expect(await seriousViolations(page)).toEqual([]);
    await checkLayout(page, "工作台出图页");
  });

  test("批量出图：两组各一张跑完，队列与画廊都有数", async ({ page, context }) => {
    test.setTimeout(90_000);
    await asMember(context);
    const errors = watchConsole(page);
    await page.goto("/workbench");
    await expect(page.locator(".nai-generate")).toBeVisible();
    await page.getByRole("button", { name: "菜单" }).click();
    await page.getByRole("tab", { name: "批量" }).click();

    const batch = page.locator('section[aria-label="批量出图"]');
    await batch.locator("textarea").nth(0).fill("batch one");
    await batch.locator("textarea").nth(1).fill("batch two");
    await expect(batch).toContainText("这次会送出 2 张");
    await page.getByRole("button", { name: "批量开始" }).click();

    await expect(batch.locator(".wb-job--done")).toHaveCount(2, { timeout: 60_000 });
    await expect(batch).toContainText("已完成 2");
    await expect(page.locator(".wb-thumb")).toHaveCount(2);

    expect(errors).toEqual([]);
    expect(await seriousViolations(page)).toEqual([]);
  });

  test("窄屏布局：375 / 320 无横向滚动，批量页签同样干净", async ({ page, context }) => {
    await asMember(context);
    await page.goto("/workbench");
    for (const w of [375, 320]) {
      await page.setViewportSize({ width: w, height: 900 });
      await expect(page.locator(".nai-generate")).toBeVisible();
      await checkLayout(page, `工作台 ${w}px`);
    }
    await page.getByRole("button", { name: "菜单" }).click();
    await page.getByRole("tab", { name: "批量" }).click();
    await checkLayout(page, "工作台批量页 375px");
  });

  test("游客访问 /workbench：只见登录引导，不出图", async ({ page }) => {
    const errors = watchConsole(page);
    await page.goto("/workbench");
    await expect(page.locator(".card__title")).toContainText("生图工作台");
    await expect(page.getByRole("link", { name: "用 Discord 登录" })).toBeVisible();
    expect(page.locator(".wb-side")).toHaveCount(0);
    expect(errors).toEqual([]);
  });
});
