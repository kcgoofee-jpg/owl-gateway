/**
 * 后台 B 组分区（上游 / 领 Key 与功能 / 额度与保护 / Discord / 系统）的端到端检查：
 * 登录门、逐个分区无控制台报错 / 无障碍 / 窄屏不横向滚动、改一项设置并保存、危险操作的确认框。
 *
 * 前置：后端（`cargo xtask dev`）+ 前端都在跑，E2E_BASE 指向前端（要用 localhost，后端按 ADMIN_ALLOWED_ORIGINS 校验 Origin）：
 *   cargo xtask dev
 *   cd web && pnpm exec vite --port 5174 --strictPort
 *   E2E_BASE=http://localhost:5174 pnpm exec playwright test e2e/admin-b.spec.ts
 * 会话直接用演示数据里生成的后台 Cookie（cookies.json，见 devEnv.ts；登录在开发环境里放宽了限流，
 * 生产是 5 分钟 5 次，所以这里只在「登录门」里真登录一次，被限流就跳过）。
 * 截图（桌面 1280 / 手机 375，浅色 / 深色）写到 /tmp/owl-shots-admin-b/，不进仓库。
 *
 * 不会点发测试告警 / 全站重置 / 清空原图 / 改密码 这类真正对外发消息或不可逆的按钮。
 * 用例会改设置再还原，所以同一文件里按顺序跑（共用配置是 fullyParallel）。
 */
import { mkdirSync } from "node:fs";
import { expect, type Page, test } from "@playwright/test";
import { readCreds } from "./devEnv";
import { noHorizontalScroll, seriousViolations, watchConsole } from "./helpers";

test.describe.configure({ mode: "default" });

const SHOTS = process.env.E2E_SHOTS ?? "/tmp/owl-shots-admin-b";
const creds = readCreds();
const PASSWORD = process.env.E2E_ADMIN_PASSWORD ?? creds.admin_password ?? "";
const COOKIE = process.env.E2E_ADMIN_COOKIE ?? creds.admin ?? "";

const SECTIONS = [
  { id: "upstream", title: "上游" },
  { id: "onboarding", title: "领 Key 与功能" },
  { id: "limits", title: "额度与保护" },
  { id: "inbox", title: "来信" },
  { id: "system", title: "系统" },
] as const;

async function visit(page: Page, id: string, title: string): Promise<void> {
  await page.goto(`/admin/${id}`);
  await expect(page.getByRole("heading", { level: 1, name: title, exact: true })).toBeVisible();
  await expect(page.locator('[aria-busy="true"]')).toHaveCount(0);
  await page.waitForTimeout(400);
}

test.beforeAll(() => {
  mkdirSync(SHOTS, { recursive: true });
});
test.beforeEach(async ({ context, baseURL }) => {
  if (COOKIE)
    await context.addCookies([
      { name: "nai_gate_admin", value: COOKIE, url: baseURL ?? "http://localhost:5176" },
    ]);
});

test("登录门：密码错误有提示，正确密码进入后台", async ({ page, context }) => {
  await context.clearCookies();
  await page.goto("/admin");
  await page.getByLabel("后台密码").fill("definitely-wrong-password");
  await page.getByRole("button", { name: "登录" }).click();
  const alert = page.getByRole("alert");
  await expect(alert).toBeVisible();
  if ((await alert.textContent())?.includes("频繁")) test.skip(true, "登录限流（5 分钟 5 次），稍后再跑");
  await expect(alert).toContainText("密码错误");
  await page.getByLabel("后台密码").fill(PASSWORD);
  await page.getByRole("button", { name: "登录" }).click();
  const nav = page.getByRole("navigation", { name: "后台导航" });
  await expect(nav.or(page.getByRole("alert").filter({ hasText: "频繁" }))).toBeVisible();
  if (!(await nav.isVisible())) test.skip(true, "登录限流（5 分钟 5 次），稍后再跑");
  await expect(nav).toBeVisible();
});

for (const s of SECTIONS) {
  test(`${s.title}：无控制台报错、无障碍、窄屏不横向滚动，并截图`, async ({ page }) => {
    const errors = watchConsole(page, "/admin/api/events");
    await page.setViewportSize({ width: 1280, height: 900 });
    await page.emulateMedia({ colorScheme: "light" });
    await visit(page, s.id, s.title);
    await page.screenshot({ path: `${SHOTS}/${s.id}-desktop-light.png`, fullPage: true });
    expect(await seriousViolations(page), "桌面浅色").toEqual([]);

    await page.emulateMedia({ colorScheme: "dark" });
    await page.waitForTimeout(200);
    await page.screenshot({ path: `${SHOTS}/${s.id}-desktop-dark.png`, fullPage: true });
    expect(await seriousViolations(page), "桌面深色").toEqual([]);

    await page.setViewportSize({ width: 375, height: 812 });
    await page.waitForTimeout(300);
    expect(await noHorizontalScroll(page), "手机宽度下页面出现横向滚动").toBe(true);
    await page.screenshot({ path: `${SHOTS}/${s.id}-mobile-dark.png`, fullPage: true });
    expect(await seriousViolations(page), "手机深色").toEqual([]);

    await page.emulateMedia({ colorScheme: "light" });
    await page.waitForTimeout(200);
    expect(await noHorizontalScroll(page)).toBe(true);
    await page.screenshot({ path: `${SHOTS}/${s.id}-mobile-light.png`, fullPage: true });
    expect(await seriousViolations(page), "手机浅色").toEqual([]);

    expect(errors).toEqual([]);
  });
}

test("字号手机与桌面一致（正文 / 卡片标题 / 提示）", async ({ page }) => {
  const sizes = async () =>
    page.evaluate(() => {
      const fs = (sel: string) => {
        const el = document.querySelector(sel);
        return el ? getComputedStyle(el).fontSize : null;
      };
      return [fs("h1"), fs(".card__title"), fs(".field__label"), fs(".field__hint"), fs(".btn")];
    });
  await page.setViewportSize({ width: 1280, height: 900 });
  await visit(page, "limits", "额度与保护");
  const desktop = await sizes();
  await page.setViewportSize({ width: 375, height: 812 });
  await page.waitForTimeout(300);
  expect(await sizes()).toEqual(desktop);
});

test("额度与保护：改一项运行限制并保存（不合法时不能保存）", async ({ page }) => {
  await visit(page, "limits", "额度与保护");
  const field = page.getByLabel("排队超时");
  const original = await field.inputValue();
  const save = page.getByRole("button", { name: "保存账号保护" });
  await expect(save).toBeDisabled();

  await field.fill("");
  await expect(page.getByText("排队超时不能为空")).toBeVisible();
  await expect(save).toBeDisabled();
  await field.fill("-5");
  await expect(page.getByText("不能是负数")).toBeVisible();
  await field.fill("9999");
  await expect(page.getByText("必须在 15～300 之间")).toBeVisible();
  await expect(save).toBeDisabled();

  const next = String(Number(original) === 91 ? 92 : 91);
  await field.fill(next);
  await expect(save).toBeEnabled();
  await save.click();
  await expect(page.getByText("运行限制已保存并生效")).toBeVisible();
  await expect(save).toBeDisabled();

  await page.reload();
  await expect(page.getByLabel("排队超时")).toHaveValue(next);
  // 还原
  await page.getByLabel("排队超时").fill(original);
  await page.getByRole("button", { name: "保存账号保护" }).click();
  await expect(page.getByText("运行限制已保存并生效").first()).toBeVisible();
});

test("额度与保护：全是手动表单 + 只读记录；没有算法说明", async ({ page }) => {
  await visit(page, "limits", "额度与保护");
  await expect(page.getByLabel("V4.5 每人每天", { exact: false })).toHaveValue("150");
  await expect(page.getByLabel("单号每天上限", { exact: false })).toHaveValue("2400");
  await expect(page.getByLabel("每人排队", { exact: false })).toHaveValue("2");
  await expect(page.getByLabel("V5 Full 转为 Medium").locator("option")).toHaveCount(2);
  await expect(page.getByRole("switch", { name: "暂停 V5" })).toBeVisible();
  await expect(page.getByTestId("guard-pace")).toBeVisible();
  await expect(page.getByTestId("quota-records")).toBeVisible();
  // 删除的自动算法不再出现
  for (const gone of ["今天的额度怎么来的", "出图间隔自动调整", "429 倍数", "动态额度"])
    await expect(page.getByText(gone, { exact: false })).toHaveCount(0);
  // 出图节奏只剩一对手动旋钮：每张出完后歇 N～M 秒；开始到开始的出图间隔、间隔抖动已删除
  await expect(page.getByLabel("每张出完后歇（最少）", { exact: false })).toHaveValue("1");
  await expect(page.getByLabel("每张出完后歇（最多）", { exact: false })).toHaveValue("3");
  for (const gone of ["出图间隔", "间隔抖动", "静默最少", "静默最多"])
    await expect(page.getByLabel(gone, { exact: false })).toHaveCount(0);
  // 429 冷却是全站的（那个账号另按上游要求单独冷却），说明要说准
  await expect(page.getByText("上游返回 429 后，全站暂停出图多久", { exact: false })).toBeVisible();
  // 改 V4.5 每人每天并还原
  const field = page.getByLabel("V4.5 每人每天", { exact: false });
  await field.fill("120");
  await page.getByRole("button", { name: "保存额度" }).click();
  await expect(page.getByText("额度已保存并同步").first()).toBeVisible();
  await field.fill("150");
  await page.getByRole("button", { name: "保存额度" }).click();
});

test("来信：问卷 / 文字反馈两个 tab；旧地址换到对应子页；没有 Discord 分区", async ({ page }) => {
  await visit(page, "inbox", "来信");
  // 侧栏 / ⌘K 落在分区根路径：换到问卷页
  await expect(page).toHaveURL(/\/admin\/inbox\/survey$/);
  const tabs = page.getByRole("navigation", { name: "来信类型" });
  await expect(tabs.getByRole("link", { name: "问卷" })).toHaveAttribute("aria-current", "page");
  // 文字反馈不再折在问卷页底部，而是第二个 tab
  await tabs.getByRole("link", { name: /文字反馈/ }).click();
  await expect(page).toHaveURL(/\/admin\/inbox\/feedback$/);
  await expect(page.getByRole("columnheader")).toHaveText([
    "时间",
    "成员",
    "内容",
    "客户端 / 请求",
    "处理状态",
  ]);
  await expect(page.getByRole("button", { name: /导出 CSV/ })).toBeVisible();
  // 旧链接（/admin/inbox/feedback、/admin/logs/feedback）都能落到反馈页
  await page.goto("/admin/inbox/feedback");
  await expect(page.getByRole("link", { name: /文字反馈/ })).toHaveAttribute("aria-current", "page");
  await page.goto("/admin/logs/feedback");
  await expect(page).toHaveURL(/\/admin\/inbox\/feedback$/);
  await expect(page.locator(".admin-nav__item", { hasText: "Discord" })).toHaveCount(0);
});

test("系统：只剩告警渠道和后台密码；没有服务器状态、定时任务", async ({ page }) => {
  await visit(page, "system", "系统");
  await expect(page.getByRole("heading", { name: "告警渠道" })).toBeVisible();
  await expect(
    page.getByText("只在重大事故时私信站长：上游 Token 失效、账号可能被封控、服务停了、磁盘快满。"),
  ).toBeVisible();
  await expect(page.getByRole("heading", { name: "后台密码" })).toBeVisible();
  for (const gone of ["服务器状态", "定时任务"])
    await expect(page.getByRole("heading", { name: gone })).toHaveCount(0);
  await expect(page.getByText("总览")).toHaveCount(0);
});

test("领 Key 与功能：改有效期并保存", async ({ page }) => {
  await visit(page, "onboarding", "领 Key 与功能");
  const field = page.getByLabel("Key 有效期");
  const original = await field.inputValue();
  const next = String(Number(original) + 1);
  await field.fill(next);
  await page.getByRole("button", { name: "保存", exact: true }).first().click();
  await expect(page.getByText("领 Key 设置已保存")).toBeVisible();
  await page.reload();
  await expect(page.getByLabel("Key 有效期")).toHaveValue(next);
  await page.getByLabel("Key 有效期").fill(original);
  await page.getByRole("button", { name: "保存", exact: true }).first().click();
  await expect(page.getByText("领 Key 设置已保存").first()).toBeVisible();

  // 高危操作只在「成员 › 更多」里，这一页不再有第二个入口
  await expect(page.getByRole("button", { name: "全站重置 Key" })).toHaveCount(0);
});

test("领 Key 与功能：领 Key 门槛一张卡——添加一条、校验 ID、一次保存、刷新还在、删除后还原", async ({
  page,
}) => {
  await visit(page, "onboarding", "领 Key 与功能");
  // 主身份组和额外身份组合成一张卡、共用一个「保存」
  const gate = page.getByRole("region", { name: "领 Key 门槛" });
  await expect(gate.getByText(/满足任意一个即可，可来自不同服务器/)).toBeVisible();
  await expect(gate.getByRole("button", { name: "保存", exact: true })).toBeDisabled();
  await gate.getByRole("button", { name: "添加一个身份组" }).click();
  const guild = gate.getByLabel("额外身份组 1 的服务器 ID");
  const role = gate.getByLabel("额外身份组 1 的身份组 ID");
  await guild.fill("123");
  await role.fill("900000000000000333");
  await expect(gate.getByRole("alert").filter({ hasText: "第 1 条：服务器 ID" })).toBeVisible();
  await guild.fill("900000000000000222");
  await gate.getByLabel("额外身份组 1 的显示名").fill("棱镜「创作者」");
  await gate.getByRole("button", { name: "保存", exact: true }).click();
  await expect(page.getByText("领 Key 门槛已保存")).toBeVisible();
  await page.reload();
  await expect(gate.getByLabel("额外身份组 1 的服务器 ID")).toHaveValue("900000000000000222");
  await expect(gate.getByLabel("额外身份组 1 的显示名")).toHaveValue("棱镜「创作者」");
  // 删掉并保存：回到没有额外身份组
  await gate.getByRole("button", { name: "删除额外身份组 1" }).click();
  await gate.getByRole("button", { name: "保存", exact: true }).click();
  await expect(page.getByText("领 Key 门槛已保存").first()).toBeVisible();
  await page.reload();
  await expect(gate.getByLabel("额外身份组 1 的服务器 ID")).toHaveCount(0);
});

test("上游：令牌只显示掩码；没有并发设置；添加 Token 校验格式", async ({ page }) => {
  await visit(page, "upstream", "上游");
  await expect(page.locator(".token__name").first()).toContainText("…");
  await expect(page.getByLabel("图片并发")).toHaveCount(0);
  await expect(page.getByRole("heading", { name: "上游表现" })).toHaveCount(0);

  await page.getByRole("button", { name: "添加 Token" }).click();
  const dialog = page.getByRole("dialog", { name: "添加上游 Token" });
  await dialog.getByLabel("NovelAI 持久 API Token").fill("not-a-token");
  await dialog.getByRole("button", { name: "添加" }).click();
  await expect(dialog.getByRole("alert")).toContainText("应以 pst- 开头");
  await expect(dialog.getByLabel("NovelAI 持久 API Token")).toHaveAttribute("type", "password");
  await dialog.getByRole("button", { name: "取消" }).click();
});

test("系统：改密码先在本地校验；清空原图 / 测试告警有保护（不真改、不真发）", async ({ page }) => {
  await visit(page, "system", "系统");
  await page.getByLabel("当前密码", { exact: true }).fill("whatever-current");
  await page.getByLabel("新密码", { exact: true }).fill("a-new-long-password");
  await page.getByLabel("确认新密码", { exact: true }).fill("a-different-password");
  await page.getByRole("button", { name: "修改密码" }).click();
  await expect(page.getByRole("alert").filter({ hasText: "两次输入的新密码不一致" })).toBeVisible();
  await page.getByLabel("新密码", { exact: true }).fill("short");
  await page.getByLabel("确认新密码", { exact: true }).fill("short");
  await page.getByRole("button", { name: "修改密码" }).click();
  await expect(page.getByText("新密码至少需要 12 个字符")).toBeVisible();
  // 显示 / 隐藏密码
  await page.getByRole("button", { name: "显示新密码" }).click();
  await expect(page.getByLabel("新密码", { exact: true })).toHaveAttribute("type", "text");

  // 没配置告警渠道时「测试告警」不可点
  await expect(page.getByRole("button", { name: "发送测试告警" })).toBeDisabled();
  // 清空原图搬到了「生成记录 › 设置」：没有原图时不可点
  await page.goto("/admin/audit/settings");
  await expect(page.getByRole("button", { name: "清空服务器上的全部原图" })).toBeDisabled();
});

test("领 Key 与功能：改名额直接保存；开放领 Key / 关功能要先确认", async ({ page }) => {
  await visit(page, "onboarding", "领 Key 与功能");
  // 改名额上限
  const seats = page.getByLabel("名额上限", { exact: true });
  const original = await seats.inputValue();
  await seats.fill(String(Number(original) + 1));
  await page.getByRole("button", { name: "保存", exact: true }).first().click();
  // 发名额公告 10-23 删除：保存就是保存，不再问要不要公告
  await expect(page.getByText("领 Key 设置已保存").first()).toBeVisible();
  await expect(page.getByRole("dialog", { name: "名额上限有变化" })).toHaveCount(0);
  await seats.fill(original);
  await page.getByRole("button", { name: "保存", exact: true }).first().click();
  await expect(page.getByLabel("名额上限", { exact: true })).toHaveValue(original);

  // 开放 / 暂停领 Key：先确认；没有「公告」选项
  await page.getByRole("switch", { name: /开放自助领取/ }).click();
  const openDialog = page.getByRole("dialog", { name: /自助领 Key？/ });
  await expect(openDialog.getByRole("button", { name: /^(开放|暂停)$/ })).toBeVisible();
  await expect(openDialog.getByRole("button", { name: /公告/ })).toHaveCount(0);
  await openDialog.getByRole("button", { name: "取消" }).click();
  await expect(openDialog).toBeHidden();

  // 功能开关：关一项再保存，要确认；取消后撤销（保存按钮按卡作用域取，中间插着「领 Key 门槛」卡）
  const feats = page.getByRole("region", { name: "功能开关" });
  const vibe = feats.getByRole("switch", { name: /Vibe 编码/ });
  const wasOn = (await vibe.getAttribute("aria-checked")) === "true";
  await vibe.click();
  await feats.getByRole("button", { name: "保存", exact: true }).click();
  const featDialog = page.getByRole("dialog", { name: "确认修改功能开关？" });
  await expect(featDialog).toBeVisible();
  await featDialog.getByRole("button", { name: "取消" }).click();
  await feats.getByRole("button", { name: "撤销修改" }).click();
  await expect(vibe).toHaveAttribute("aria-checked", String(wasOn));
});
