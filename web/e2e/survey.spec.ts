import { expect, test } from "@playwright/test";
import { COOKIES_FILE, readCreds } from "./devEnv";
import { asMember, ME, noHorizontalScroll, patch, STATUS, seriousViolations, watchConsole } from "./helpers";

/** 问卷页端到端：用 Discord 会话识别身份，提交时不带 Key。配置见 playwright.config.ts。 */

const member = readCreds();
const needMember = !member.member;

test.describe("游客", () => {
  test("看到登录入口而不是问卷；点登录前记下要回到问卷", async ({ page }) => {
    const errors = watchConsole(page);
    await patch(page, STATUS, (b) => ({ ...b, web_login: true }));
    await page.route("**/login", (r) => r.fulfill({ contentType: "text/html", body: "<p>登录</p>" }));
    await page.goto("/survey");
    await expect(page.getByRole("heading", { level: 1, name: "成员问卷" })).toBeVisible();
    await expect(page.getByRole("heading", { name: "请先登录" })).toBeVisible();
    await expect(page.getByRole("group")).toHaveCount(0);
    await expect(page.getByRole("button", { name: "提交问卷" })).toHaveCount(0);
    // 不再有 Key 粘贴框
    await expect(page.getByPlaceholder("nai-…")).toHaveCount(0);
    await page.getByRole("main").getByRole("link", { name: "用 Discord 登录" }).click();
    await page.waitForURL("**/login");
    expect(await page.evaluate(() => sessionStorage.getItem("owl-next"))).toBe("/survey");
    expect(errors).toEqual([]);
  });

  test("手机 375px 无横向滚动", async ({ page }) => {
    await page.setViewportSize({ width: 375, height: 812 });
    await patch(page, STATUS, (b) => ({ ...b, web_login: true }));
    await page.goto("/survey");
    await expect(page.getByRole("heading", { name: "请先登录" })).toBeVisible();
    expect(await noHorizontalScroll(page)).toBe(true);
  });
});

test.describe("登录成员", () => {
  test.skip(needMember, `没有成员会话文件 ${COOKIES_FILE}（先跑 cargo xtask dev）`);
  test.beforeEach(async ({ context }) => asMember(context));

  test("题目：没有设备题；客户端必答，分组有 legend，输入有 label", async ({ page }) => {
    const errors = watchConsole(page);
    await page.goto("/survey");
    await expect(page.getByRole("heading", { level: 1, name: "成员问卷" })).toBeVisible();
    const q1 = page.getByLabel(/1\. 你主要用哪个客户端接入/);
    await expect(q1).toBeVisible();
    await expect(page.locator(".field__label", { hasText: "1. 你主要用哪个客户端接入" })).toContainText(
      "必答",
    );
    const q2 = page.getByRole("group", { name: /2\. 你最希望改进哪些地方/ });
    await expect(page.getByText("你平时用什么设备出图")).toHaveCount(0);
    await expect(page.getByText("限额的原理和初衷")).toHaveCount(0);
    await expect(q2.getByRole("checkbox")).toHaveCount(6);
    await expect(page.getByRole("group", { name: /3\./ })).toHaveCount(0);
    await expect(q2.getByLabel("其他想法（选填）")).toBeVisible();
    // 一个会话一份：不再要求粘贴 Key
    await expect(page.getByPlaceholder("nai-…")).toHaveCount(0);
    expect(errors).toEqual([]);
  });

  test("没答必答题：逐题提示并把焦点放到第一个没答的题", async ({ page }) => {
    await page.goto("/survey");
    await page.getByRole("button", { name: "提交问卷" }).click();
    const q1 = page.getByLabel(/1\. 你主要用哪个客户端接入/);
    await expect(page.getByRole("alert").filter({ hasText: "第 1 题还没填" })).toBeVisible();
    await expect(q1).toBeFocused();
    // 填了就消掉
    await q1.fill("柏宝绘 1.2");
    await expect(page.getByRole("alert").filter({ hasText: "第 1 题还没填" })).toHaveCount(0);
  });

  test("提交：只靠 Discord 会话（不带 Authorization），请求体字段与旧版一致，成功后提示", async ({
    page,
  }) => {
    await page.goto("/survey");
    await page.getByLabel(/1\. 你主要用哪个客户端接入/).fill("柏宝绘 1.2");
    await page.getByLabel("稳定性（少报错、少失败）").check();
    await page.getByLabel("教程和文档（接入步骤、常见问题）").check();
    await page.getByLabel("其他想法（选填）").fill("e2e 测试，请忽略");

    const reqPromise = page.waitForRequest("**/member/survey");
    const resPromise = page.waitForResponse("**/member/survey");
    await page.getByRole("button", { name: "提交问卷" }).click();
    const req = await reqPromise;
    expect(req.method()).toBe("POST");
    expect(await req.headerValue("authorization")).toBeNull();
    expect(req.postDataJSON()).toEqual({
      client: "柏宝绘 1.2",
      improve: ["稳定性", "教程和文档"],
      improve_other: "e2e 测试，请忽略",
    });
    expect((await resPromise).status()).toBe(200);
    await expect(page.getByRole("status").filter({ hasText: "已提交，谢谢你的反馈！" })).toBeVisible();
    await expect(page.getByRole("button", { name: "更新我的回答" })).toBeVisible();
  });

  test("服务端拒绝时显示原因（401：登录过期）", async ({ page }) => {
    await page.route("**/member/survey", (r) => r.fulfill({ status: 401, json: { detail: "Key 无效" } }));
    await page.goto("/survey");
    await page.getByLabel(/1\. 你主要用哪个客户端接入/).fill("柏宝绘 1.2");
    await page.getByRole("button", { name: "提交问卷" }).click();
    await expect(page.getByRole("alert").filter({ hasText: "重新登录" })).toBeVisible();
    await expect(page.getByRole("button", { name: "提交问卷" })).toBeEnabled();
  });

  test("已登录但还没有 Key：不显示题目，提示领取方式", async ({ page }) => {
    await patch(page, ME, (b) => ({ ...b, has_key: false, key: undefined }));
    await page.goto("/survey");
    await expect(page.getByText("你还没有 Key，所以暂时不能填写问卷")).toBeVisible();
    await expect(page.getByRole("group")).toHaveCount(0);
  });

  test("键盘：聚焦选项时整行有可见的焦点环", async ({ page }) => {
    await page.goto("/survey");
    await page.getByLabel("稳定性（少报错、少失败）").focus();
    const outline = await page
      .getByLabel("稳定性（少报错、少失败）")
      .evaluate((el) => getComputedStyle(el.closest("label") as HTMLElement).outlineStyle);
    expect(outline).not.toBe("none");
  });

  test("手机 375px 无横向滚动", async ({ page }) => {
    await page.setViewportSize({ width: 375, height: 812 });
    await page.goto("/survey");
    await expect(page.getByRole("button", { name: "提交问卷" })).toBeVisible();
    expect(await noHorizontalScroll(page)).toBe(true);
  });

  test("普通成员进首页：不会自动弹问卷", async ({ page }) => {
    await page.goto("/");
    await page.getByRole("main").waitFor();
    await expect(page.locator("dialog[open]")).toHaveCount(0);
    await page.goto("/?survey=1");
    await page.getByRole("main").waitFor();
    await expect(page.locator("dialog[open]")).toHaveCount(0);
  });

  for (const scheme of ["light", "dark"] as const) {
    test(`无障碍（axe）：${scheme} 没有严重 / 致命问题`, async ({ page }) => {
      await page.emulateMedia({ colorScheme: scheme });
      await page.goto("/survey");
      await page.getByRole("button", { name: "提交问卷" }).waitFor();
      expect(await seriousViolations(page)).toEqual([]);
      // 带着错误提示再测一遍
      await page.getByRole("button", { name: "提交问卷" }).click();
      await page.getByRole("alert").first().waitFor();
      expect(await seriousViolations(page)).toEqual([]);
    });
  }
});
