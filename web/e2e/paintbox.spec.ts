/**
 * 画匣（Beta）端到端：真实栈（cargo xtask dev 的假上游）里走一遍完整生成，
 * 再各演一条空提示词拦截与网关报错。成员会话来自 cookies.json，没有就整组跳过。
 */
import { expect, test } from "@playwright/test";
import { readCreds } from "./devEnv";
import { asMember, noHorizontalScroll, seriousViolations, watchConsole } from "./helpers";

const hasMember = Boolean(readCreds().member);

test.describe("画匣（Beta）", () => {
  test.skip(!hasMember, "需要成员会话（cargo xtask dev 的 cookies.json）");

  test("成员首页出现画匣，真实假上游生成一张图并展示", async ({ page, context }) => {
    await asMember(context);
    const errors = watchConsole(page);
    await page.goto("/");
    const paintbox = page.locator("#paintbox");
    await expect(paintbox).toBeVisible();
    await expect(paintbox.locator(".badge")).toHaveText("Beta");
    // 提示都在：写法、计费口径、模型清单里有 V5（演示数据开着 V5）
    await expect(paintbox).toContainText("英文标签、逗号分隔");
    await expect(paintbox).toContainText("计入今天的");
    await expect(paintbox.locator("select").first()).toContainText("V5 Full");

    await paintbox.locator("textarea").first().fill("1girl, seaside, sunset");
    await paintbox.getByRole("button", { name: "生成" }).click();
    // 假上游回装着真 PNG 的 zip：走完网关排队 / 记账，图出现在预览里
    await expect(paintbox.locator(".pb-shot__img img")).toBeVisible({ timeout: 30_000 });
    await expect(paintbox.locator(".pb-shot__actions")).toContainText("下载");

    // 点开大图：属性面板有种子和提示词
    await paintbox.locator(".pb-shot__img").click();
    const dialog = page.locator("dialog[open]");
    await expect(dialog).toContainText("种子");
    await expect(dialog).toContainText("提示词");
    await dialog.getByRole("button", { name: "关闭" }).click();
    await expect(dialog).toBeHidden();

    expect(errors).toEqual([]);
    expect(await seriousViolations(page)).toEqual([]);
    expect(await noHorizontalScroll(page)).toBe(true);
  });

  test("空提示词拦下不发请求；网关报错原样展示", async ({ page, context }) => {
    await asMember(context);
    await page.goto("/");
    const paintbox = page.locator("#paintbox");
    await expect(paintbox).toBeVisible();

    let sent = 0;
    page.on("request", (req) => {
      if (req.url().includes("/ai/generate-image") && req.method() === "POST") sent++;
    });
    await paintbox.getByRole("button", { name: "生成" }).click();
    await expect(paintbox.locator(".notice--danger")).toContainText("先写点提示词");
    expect(sent).toBe(0);

    await page.route(/\/ai\/generate-image$/, (route) =>
      route.fulfill({
        status: 403,
        contentType: "application/json",
        body: JSON.stringify({
          statusCode: 403,
          error: { message: "这把 Key 已被临时暂停，未扣费。稍后自动恢复。", status: 403 },
        }),
      }),
    );
    await paintbox.locator("textarea").first().fill("1girl");
    await paintbox.getByRole("button", { name: "生成" }).click();
    await expect(paintbox.locator(".notice--danger")).toContainText("临时暂停");
    expect(sent).toBe(1);
  });

  test("游客看不到画匣", async ({ browser }) => {
    const guest = await browser.newContext();
    const page = await guest.newPage();
    await page.goto("/");
    await expect(page.locator("#paintbox")).toHaveCount(0);
    await guest.close();
  });
});
