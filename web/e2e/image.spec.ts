/**
 * 官网镜像页（/image）：跑的是 NovelAI 官方构建产物，后端换成本站网关。
 * 断言只依赖官网自己的 DOM 语义（不是我们造的选择器）。
 */
import { expect, test } from "@playwright/test";
import { readCreds } from "./devEnv";
import { asMember, watchConsole } from "./helpers";

const hasMember = Boolean(readCreds().member);
const BASE = process.env.E2E_BASE ?? "http://localhost:5174";

/** 官网首次登录会弹许可/订阅/教程层，测试里点掉；都不在就直接过。 */
async function dismissFirstRun(page: import("@playwright/test").Page) {
  for (const text of [
    "Got it!",
    "I agree with the above terms",
    "Accept",
    "Don't show this again.",
    "I know what I’m doing, skip this!",
  ]) {
    const btn = page.getByText(text, { exact: true }).last();
    if (await btn.count()) await btn.click({ timeout: 2000 }).catch(() => {});
  }
}

test.describe("官网镜像页", () => {
  test.skip(!hasMember, "需要成员会话（cargo xtask dev 的 cookies.json）");

  test("/workbench 跳到 /image，并且是登录态", async ({ page, context }) => {
    await asMember(context, BASE);
    const errors = watchConsole(page);
    // 官网 bundle 会自己注入统计/支付/验证码脚本，我们在 CSP 里主动挡掉，控制台里的 CSP 拦截不算失败
    const blocked = (line: string) =>
      /ph-fwd\.novelai\.net|googletagmanager|chargebee|recaptcha|accounts\.google\.com|doubleclick|alb\.reddit\.com/.test(
        line,
      );
    await page.goto("/workbench");
    await expect(page).toHaveURL(/\/image$/);

    // 官网外壳：模型行、提示词标签、生成按钮
    await expect(page.getByText("Model", { exact: true })).toBeVisible({ timeout: 20_000 });
    await expect(page.getByRole("button", { name: /Generate \d+ Image/ })).toBeVisible();
    // 登录态：官网未登录时会挂「Create an account」横幅，这里必须没有
    await expect(page.getByText("Create an account and start for free")).toHaveCount(0);
    expect(errors.filter((e) => !blocked(e))).toEqual([]);
  });

  test("真实出图：一张图进画布和历史轨", async ({ page, context }) => {
    test.setTimeout(180_000);
    await asMember(context, BASE);
    await page.goto("/image");
    await dismissFirstRun(page);

    // 提示词编辑器是 ProseMirror；点进去再输入，输入成功才算数
    const editor = page.locator(".ProseMirror").first();
    await editor.click();
    await page.keyboard.insertText("1girl, seaside, sunset");
    await expect
      .poll(async () => (await editor.innerText()).trim().length, { timeout: 10_000 })
      .toBeGreaterThan(0);

    const stream = page.waitForResponse((r) => /\/ai\/generate-image(-stream)?$/.test(r.url()), {
      timeout: 120_000,
    });
    await page
      .locator("button", { hasText: /Generate \d+ Image/ })
      .first()
      .click();
    const response = await stream;
    expect(response.status(), "网关的出图接口要回 200").toBe(200);
    await dismissFirstRun(page);

    // 画布出现生成结果（官网把生成图渲染成 data:/blob: 图），历史轨同时多一条缩略图
    const generated = page.locator('img[src^="data:"], img[src^="blob:"]').first();
    await expect(generated).toBeVisible({ timeout: 120_000 });
    const box = await generated.boundingBox();
    expect(box?.width ?? 0).toBeGreaterThan(200);
  });
});
