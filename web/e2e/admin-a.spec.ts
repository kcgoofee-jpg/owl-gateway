/**
 * 后台「成员 / 日志」端到端：登录 → 逐页打开 → 打开 Key 编辑框 → 无控制台报错 →
 * 手机 375px 不横向滚动 → axe 无严重 / 致命问题 → 截图（浅色 / 深色 × 桌面 / 手机）。
 *
 * 运行（先起后端和 vite，见 playwright.config.ts 顶部：`cargo xtask dev` + `pnpm exec vite --port 5174`）：
 *   E2E_BASE=http://localhost:5174 pnpm exec playwright test e2e/admin-a.spec.ts
 * 后台密码取自环境变量 E2E_ADMIN_PASSWORD，或 `cargo xtask dev` 写出的 cookies.json（见 devEnv.ts，E2E_COOKIES 可改位置）。
 * 截图用例只在设置了 E2E_SHOTS=<目录> 时才跑，截图不进仓库。
 */
import { mkdirSync } from "node:fs";
import { type Cookie, expect, type Page, test } from "@playwright/test";
import { COOKIES_FILE, readCreds } from "./devEnv";
import { adminSession, noHorizontalScroll, seriousViolations, watchConsole } from "./helpers";
import { orphans } from "./orphans";

const SHOTS = process.env.E2E_SHOTS ?? "";
const DESKTOP = { width: 1280, height: 900 };
const MOBILE = { width: 375, height: 812 };

function adminPassword(): string {
  const password = process.env.E2E_ADMIN_PASSWORD ?? readCreds().admin_password;
  if (password) return password;
  throw new Error(
    `找不到后台密码：设置 E2E_ADMIN_PASSWORD，或先跑 cargo xtask dev（会写出 ${COOKIES_FILE}）`,
  );
}

let SESSION: Cookie[] = [];

/** 每个页面：路径、就绪标志（等到这个元素出现才算加载完） */
const PAGES: Array<{ name: string; path: string; ready: (p: Page) => ReturnType<Page["locator"]> }> = [
  {
    name: "members",
    path: "/admin/members",
    ready: (p) => p.getByRole("button", { name: /展开.*的详情/ }).first(),
  },
  {
    name: "logs-usage",
    path: "/admin/logs/usage",
    ready: (p) => p.getByText("暂无记录").or(p.locator(".lg-listcard")),
  },
  { name: "logs-actions", path: "/admin/logs/actions", ready: (p) => p.getByLabel("发起者") },
];

async function open(
  page: Page,
  path: string,
  ready: (p: Page) => ReturnType<Page["locator"]>,
): Promise<void> {
  await page.goto(path);
  await expect(ready(page).first()).toBeVisible();
}

// 后台限制登录尝试次数（5 次 / 5 分钟），所以只登录一次，把会话存下来各测试复用
// 一份会话、数据会改动（黑名单 / 标签 / 邀请 Key）：串行跑
test.describe.configure({ mode: "serial" });

test.beforeAll(async ({ browser, baseURL }) => {
  SESSION = await adminSession(browser, baseURL, adminPassword());
});

test("登录表单：错误密码有提示，正确密码进入后台", async ({ page }) => {
  // 未登录时 /me 与错误密码的 401 是预期的，只检查脚本异常
  const errs: string[] = [];
  page.on("pageerror", (e) => errs.push(e.message));
  await page.goto("/admin");
  await page.getByLabel("后台密码").fill("definitely-wrong-password");
  await page.getByRole("button", { name: "登录", exact: true }).click();
  await expect(page.getByRole("alert")).toBeVisible();
  await page.getByLabel("后台密码").fill(adminPassword());
  await page.getByRole("button", { name: "登录", exact: true }).click();
  await expect(page.getByRole("navigation", { name: "后台导航" })).toBeVisible();
  expect(errs).toEqual([]);
});

test.describe("已登录", () => {
  test.beforeEach(async ({ context }) => {
    await context.addCookies(SESSION);
  });

  test("逐页打开：无控制台报错", async ({ page }) => {
    const errs = watchConsole(page, "/admin/api/events");
    for (const p of PAGES) {
      await open(page, p.path, p.ready);
      await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
    }
    expect(errs).toEqual([]);
  });

  test("未知后台路径：404 视图并保留侧栏导航，不再静默渲染成员页", async ({ page }) => {
    await page.goto("/admin/nope");
    await expect(page.getByRole("heading", { level: 1, name: "页面不存在" })).toBeVisible();
    await expect(page.getByRole("navigation", { name: "后台导航" })).toBeVisible();
    await expect(page.getByRole("button", { name: /展开.*的详情/ })).toHaveCount(0);
    await page.getByRole("button", { name: "回到成员页" }).click();
    await expect(page).toHaveURL(/\/admin\/members$/);
    await expect(page.getByRole("button", { name: /展开.*的详情/ }).first()).toBeVisible();
  });

  test("鉴权探针网络失败：留在后台壳显示重试，不回登录页", async ({ page }) => {
    await page.route("**/admin/api/me", (route) => route.abort());
    await page.goto("/admin/members");
    await expect(page.getByRole("heading", { level: 1, name: "后台数据读取失败" })).toBeVisible();
    await expect(page.getByLabel("后台密码")).toHaveCount(0);
    await expect(page.getByRole("navigation", { name: "后台导航" })).toBeVisible();
    // 网络恢复后点「重试」：不用重新登录
    await page.unroute("**/admin/api/me");
    await page.getByRole("button", { name: "重试" }).click();
    await expect(page.getByRole("heading", { level: 1, name: "成员" })).toBeVisible();
  });

  test("长表单弹窗：900px 视口下 footer 的提交按钮可见，内容区在框内滚动", async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 900 });
    await open(page, "/admin/members", PAGES[0]?.ready as never);
    await page
      .getByRole("button", { name: /展开.*的详情/ })
      .first()
      .click();
    await page.getByRole("button", { name: "成员例外", exact: true }).click();
    const dlg = page.getByRole("dialog", { name: /成员例外/ });
    await expect(dlg).toBeVisible();
    await expect(dlg.getByRole("button", { name: "保存", exact: true })).toBeInViewport();
    const layout = await dlg.evaluate((el) => ({
      display: getComputedStyle(el).display,
      bodyOverflow: getComputedStyle(el.querySelector(".dialog__body") as HTMLElement).overflowY,
    }));
    expect(layout.display).toBe("flex");
    expect(layout.bodyOverflow).toBe("auto");
    await page.keyboard.press("Escape");
  });

  test("成员：展开详情、Key 编辑框校验（空值 / 负数 / Anlas 规则）", async ({ page }) => {
    const errs = watchConsole(page, "/admin/api/events");
    await open(page, "/admin/members", PAGES[0]?.ready as never);
    await page
      .getByRole("button", { name: /展开.*的详情/ })
      .first()
      .click();
    await page.getByRole("button", { name: "成员例外", exact: true }).click();
    const dlg = page.getByRole("dialog", { name: /成员例外/ });
    const images = dlg.getByLabel("V4.5 免费图/天", { exact: true });
    const changed = (await images.inputValue()) === "33" ? 34 : 33;
    await images.fill("");
    await expect(dlg.getByRole("button", { name: "保存", exact: true })).toBeDisabled();
    await images.fill("-3");
    await expect(dlg.getByRole("button", { name: "保存", exact: true })).toBeDisabled();
    await images.fill(String(changed));
    const saveResponse = page.waitForResponse(
      (response) =>
        /\/admin\/api\/members\/\d+\/rules$/.test(new URL(response.url()).pathname) &&
        response.request().method() === "PUT",
    );
    await dlg.getByRole("button", { name: "保存", exact: true }).click();
    const saved = await saveResponse;
    expect(saved.status()).toBe(200);
    expect((await saved.json()).effective.daily_images).toBe(changed);
    await expect(dlg).toBeHidden();
    expect(await seriousViolations(page), "member-rules-dialog").toEqual([]);
    // 已保存后再编辑一项：取消时先确认放弃。
    await page.getByRole("button", { name: "成员例外" }).click();
    await expect(dlg).toBeVisible();
    await dlg.getByLabel("V4.5 免费图/天", { exact: true }).fill("120");
    await dlg.getByRole("button", { name: "取消" }).click();
    await expect(page.getByText("放弃未保存的修改？")).toBeVisible();
    await page.getByRole("button", { name: "放弃修改" }).click();
    await expect(dlg).toBeHidden();
    expect(errs).toEqual([]);
  });

  /** 拦下成员接口，在真实数据上改一改（让列表里的极端情形可预期） */
  async function mockMembers(page: Page, edit: (body: { members: Array<Record<string, unknown>> }) => void) {
    await page.route("**/admin/api/members", async (route) => {
      const res = await route.fetch();
      const body = await res.json();
      edit(body);
      await route.fulfill({ response: res, json: body });
    });
  }
  const bar = (page: Page) => page.getByRole("group", { name: "筛选成员" });

  for (const size of [DESKTOP, MOBILE]) {
    test(`成员：筛选横栏在 ${size.width}px 是单行，不换行、不出孤字、不撑出横向滚动`, async ({ page }) => {
      await page.setViewportSize(size);
      await mockMembers(page, (body) => {
        // 最长的情形：每颗标签都带人数
        const m = body.members[0];
        if (m) m.requests_today = 9;
      });
      await open(page, "/admin/members", PAGES[0]?.ready as never);
      const b = bar(page);
      const geo = await b.evaluate((el) => {
        const chips = [...el.querySelectorAll<HTMLElement>("button")].map((x) => x.getBoundingClientRect());
        const scroll = el.querySelector<HTMLElement>(".mb-bar__scroll");
        return {
          tops: [...new Set(chips.map((r) => Math.round(r.top)))],
          heights: [...new Set(chips.map((r) => Math.round(r.height)))],
          n: chips.length,
          fits: !scroll || scroll.scrollWidth <= scroll.clientWidth + 1,
          noWrap: [...el.querySelectorAll<HTMLElement>("button")].every(
            (x) => getComputedStyle(x).whiteSpace === "nowrap",
          ),
          pageScroll: document.documentElement.scrollWidth <= window.innerWidth + 1,
        };
      });
      expect(geo.n).toBeGreaterThanOrEqual(5);
      expect(geo.tops, "所有标签同一行").toHaveLength(1);
      expect(geo.heights, "标签等高").toHaveLength(1);
      expect(geo.noWrap).toBe(true);
      expect(geo.pageScroll, "页面不横向滚动").toBe(true);
      // 放不下（手机）时标签那一段自己横向滚，而不是换行
      if (!geo.fits)
        expect(
          await b.evaluate((el) => {
            const s = el.querySelector<HTMLElement>(".mb-bar__scroll");
            return s ? getComputedStyle(s).overflowX : "visible";
          }),
        ).toBe("auto");
      expect(await orphans(page)).toEqual([]);
      expect(await seriousViolations(page), `members-bar@${size.width}`).toEqual([]);
    });
  }

  test("成员：黑名单（添加 → 搜索 → 移出）与加标签", async ({ page }) => {
    await open(page, "/admin/members", PAGES[0]?.ready as never);
    await page.getByRole("button", { name: "更多", exact: true }).first().click();
    await page.getByRole("menuitem", { name: /封禁黑名单/ }).click();
    const bans = page.getByRole("dialog", { name: "封禁黑名单" });
    await bans.getByLabel("Discord ID（纯数字）").fill("123");
    await bans.getByRole("button", { name: "加入黑名单" }).click();
    await expect(bans.getByText("Discord ID 必须是 17～20 位纯数字")).toBeVisible();
    await bans.getByLabel("Discord ID（纯数字）").fill("900000000000009999");
    await bans.getByLabel("封禁理由").fill("端到端测试");
    await bans.getByRole("button", { name: "加入黑名单" }).click();
    await expect(bans.getByText("900000000000009999").first()).toBeVisible();
    expect(await seriousViolations(page), "bans-dialog").toEqual([]);
    await bans.getByRole("button", { name: "移出黑名单" }).first().click();
    await page
      .getByRole("dialog", { name: "移出黑名单" })
      .getByRole("button", { name: "移出黑名单" })
      .click();
    await expect(bans.getByText("900000000000009999")).toHaveCount(0);
    await bans.getByRole("button", { name: "关闭", exact: true }).last().click();

    await page
      .getByRole("button", { name: /展开.*的详情/ })
      .first()
      .click();
    await page
      .getByRole("button", { name: /^标签$/ })
      .first()
      .click();
    const tag = page.getByRole("dialog", { name: /添加标签/ });
    await tag.getByRole("button", { name: "添加" }).click();
    await expect(tag.getByText("标签名不能为空")).toBeVisible();
    await tag.getByLabel("标签名").fill("端到端");
    await tag.getByRole("button", { name: "添加" }).click();
    await expect(tag).toBeHidden();
    const remove = page.getByRole("button", { name: "移除标签 端到端" });
    await expect(remove).toBeVisible();
    await remove.click();
    await expect(remove).toBeHidden();
  });

  test("成员：凭据、例外、发券、额度重置和高危确认", async ({ page }) => {
    const errs = watchConsole(page, "/admin/api/events");
    await open(page, "/admin/members", PAGES[0]?.ready as never);
    // 「全部成员」筛选：停用后这位成员不会从列表里消失
    await page
      .getByRole("group", { name: "筛选成员" })
      .getByRole("button", { name: /^全部成员/ })
      .click();
    const btn = page.getByRole("button", { name: /展开.*的详情/ }).first();
    const detailId = await btn.getAttribute("aria-controls");
    expect(detailId).toBeTruthy();
    await btn.click();
    const row = page.locator(`#${detailId}`);
    await expect(row).toBeVisible();

    await expect(row.getByRole("button", { name: "普通", exact: true })).toHaveCount(0);
    await expect(row.getByRole("button", { name: "邀请", exact: true })).toHaveCount(0);
    // 显示 Key（默认遮住中间）
    await expect(row.locator(".mb-key__v")).toContainText("····");
    await row.getByRole("button", { name: "显示" }).click();
    await expect(row.locator(".mb-key__v")).not.toContainText("····");

    // 成员例外与凭据到期时间分开。
    await row.getByRole("button", { name: "成员例外", exact: true }).click();
    const edit = page.getByRole("dialog", { name: /成员例外/ });
    await expect(edit.getByRole("button", { name: "保存", exact: true })).toBeDisabled();
    await edit.getByRole("button", { name: "取消", exact: true }).click();
    await expect(edit).toBeHidden();
    // 「请求」= 这位成员（全部 Key）的请求日志；详情里降成了文字链接（站内链接样式）
    await expect(row.getByText("最近来源")).toHaveCount(0);
    await expect(row.getByRole("link", { name: "请求", exact: true })).toBeVisible();
    await expect(row.getByRole("link", { name: "提示词记录", exact: true })).toHaveCount(0);

    // 发重置券：天数 1～30
    await row.getByRole("button", { name: "发重置券" }).click();
    const coupon = page.getByRole("dialog", { name: /发重置券/ });
    await coupon.getByLabel("有效天数").fill("0");
    await coupon.getByRole("button", { name: "发出" }).click();
    await expect(coupon.getByText("有效期必须在 1～30 之间")).toBeVisible();
    // 默认只重置 V4.5：V4.5 固定勾上，V5 / Anlas 不勾
    await expect(coupon.getByRole("checkbox", { name: /V4\.5/ })).toBeChecked();
    await expect(coupon.getByRole("checkbox", { name: /V4\.5/ })).toBeDisabled();
    await expect(coupon.getByRole("checkbox", { name: "V5" })).not.toBeChecked();
    await expect(coupon.getByRole("checkbox", { name: "Anlas" })).not.toBeChecked();
    await coupon.getByLabel("有效天数").fill("5");
    await coupon.getByRole("button", { name: "发出" }).click();
    await expect(coupon).toBeHidden();

    // 重置今日额度
    await row.getByRole("button", { name: /重置今日额度/ }).click();
    const reset = page.getByRole("dialog", { name: /重置.*今日额度/ });
    await expect(reset.getByRole("checkbox", { name: "V5" })).not.toBeChecked();
    await expect(reset.getByRole("checkbox", { name: "Anlas" })).not.toBeChecked();
    await reset.getByRole("checkbox", { name: "V5" }).check();
    const sent = page.waitForRequest((r) => r.url().includes("/reset-daily-image-quota"));
    await reset.getByRole("button", { name: "重置", exact: true }).click();
    expect((await sent).postDataJSON()).toEqual({ scope: ["v5"] });
    await expect(reset).toBeHidden();

    // 重新生成 Key：确认后显示新 Key
    await row.getByRole("button", { name: "更多" }).click();
    await page.getByRole("menuitem", { name: /重新生成 Key/ }).click();
    await page
      .getByRole("dialog", { name: /重新生成/ })
      .getByRole("button", { name: "重新生成", exact: true })
      .click();
    const reveal = page.getByRole("dialog", { name: "Key 已重新生成" });
    await expect(reveal.getByText(/^nai-/)).toBeVisible();
    await reveal.getByRole("button", { name: "完成" }).click();

    // 凭据只提供到期编辑，不提供手动绑定。
    await expect(row.getByRole("button", { name: /^(换绑|手动绑定)$/ })).toHaveCount(0);
    await expect(page.getByRole("button", { name: "新建 Key", exact: true })).toHaveCount(0);
    // 全站重置：确认词
    await page.getByRole("button", { name: "更多", exact: true }).first().click();
    await page.getByRole("menuitem", { name: /全站重置 Key/ }).click();
    const all = page.getByRole("dialog", { name: "全站重置 Key" });
    await expect(all.getByRole("button", { name: "确认重置" })).toBeDisabled();
    await all.getByRole("button", { name: "取消" }).click();

    await expect(page.getByRole("button", { name: "创建邀请 Key" })).toHaveCount(0);
    expect(errs).toEqual([]);
  });

  test("日志：子页是路由，筛选条件在地址栏里，可直接打开", async ({ page }) => {
    await page.goto("/admin/logs");
    await expect(page).toHaveURL(/\/admin\/logs\/usage$/);
    await open(page, "/admin/logs/usage?feature=image&per=50", PAGES[1]?.ready as never);
    await expect(page.getByRole("combobox", { name: /^功能/ })).toHaveValue("image");
    await expect(page.getByRole("combobox", { name: /^每页/ })).toHaveValue("50");
    await expect(page.getByRole("link", { name: "请求日志" })).toHaveAttribute("aria-current", "page");
    await expect(page.locator(".lg-sum")).toContainText(/^共 [\d,]+/);
    // 状态和原因在地址栏里；日志页不再提供 IP 筛选
    await open(page, "/admin/logs/usage?status=rejected&reason=queue_full", PAGES[1]?.ready as never);
    await expect(page.getByRole("combobox", { name: /^状态/ })).toHaveValue("rejected");
    await expect(page.getByRole("combobox", { name: /^原因/ })).toHaveValue("queue_full");
    await expect(page.getByRole("searchbox", { name: /^IP/ })).toHaveCount(0);
    await page.getByRole("combobox", { name: /^状态/ }).selectOption("ok");
    await expect(page).toHaveURL(/status=ok/);
    await expect(page).not.toHaveURL(/reason=/);
    await expect(page.getByRole("combobox", { name: /^原因/ })).toHaveCount(0);
    // 展开一行看编号 / 上游账号 / 完整 UA
    await open(page, "/admin/logs/usage", PAGES[1]?.ready as never);
    const exp = page.getByRole("button", { name: "展开" }).first();
    if (await exp.isVisible()) {
      await exp.click();
      await expect(page.locator(".lg-detail-row").first()).toContainText("上游账号");
      // 来源 IP 默认打码，点「显示」才展开完整 IP；展开后仍能按 IP 筛
      const ipCell = page
        .locator(".lg-ipis")
        .filter({ has: page.locator(".lg-iptoggle") })
        .first();
      if (await ipCell.isVisible()) {
        await expect(ipCell.locator(".lg-ip")).toContainText("*");
        await ipCell.getByRole("button", { name: /显示.*完整 IP/ }).click();
        const fullIp = (await ipCell.locator(".lg-ip").textContent()) ?? "";
        expect(fullIp).not.toContain("*");
        await ipCell.locator(".lg-ip").click();
        await expect(page.getByRole("searchbox", { name: /^IP/ })).toHaveValue(fullIp);
      }
    }
  });

  test("提示词记录已删除：旧链接回到请求日志，接口返回 404", async ({ page }) => {
    await page.goto("/admin/logs/prompts");
    await expect(page).toHaveURL(/\/admin\/logs\/usage$/);
    await expect(page.getByRole("navigation", { name: "日志类型" }).getByRole("link")).toHaveCount(2);
    await expect(page.getByRole("link", { name: "提示词记录", exact: true })).toHaveCount(0);
    expect((await page.request.get("/admin/api/prompts")).status()).toBe(404);
  });

  for (const [vp, size] of [
    ["桌面 1280", DESKTOP],
    ["手机 375", MOBILE],
  ] as const) {
    test(`${vp}：各页无横向滚动、无严重无障碍问题`, async ({ page }) => {
      await page.setViewportSize(size);
      const errs = watchConsole(page, "/admin/api/events");
      for (const p of PAGES) {
        await open(page, p.path, p.ready);
        await page.waitForTimeout(300);
        expect(await noHorizontalScroll(page, { loose: true }), `${p.name} 在 ${vp} 下出现横向滚动`).toBe(
          true,
        );
        expect(await seriousViolations(page), `${p.name}@${vp}`).toEqual([]);
      }
      if (size.width === MOBILE.width) {
        // 成员表在手机上变成卡片
        await open(page, "/admin/members", PAGES[0]?.ready as never);
        await expect(page.getByRole("list", { name: "成员列表" })).toBeVisible();
        await expect(page.locator("table.mb-table")).toHaveCount(0);
        // 展开详情、打开编辑框，仍不横向滚动
        await page
          .getByRole("button", { name: /展开.*的详情/ })
          .first()
          .click();
        expect(await noHorizontalScroll(page, { loose: true })).toBe(true);
        await page.getByRole("button", { name: "成员例外" }).click();
        await expect(page.getByRole("dialog", { name: /成员例外/ })).toBeVisible();
        expect(await noHorizontalScroll(page, { loose: true })).toBe(true);
      } else {
        await open(page, "/admin/members", PAGES[0]?.ready as never);
        await expect(page.locator("table.mb-table")).toBeVisible();
      }
      expect(errs).toEqual([]);
    });
  }

  test("字号手机与桌面一致（只有布局变化）", async ({ page }) => {
    const sizes: Record<string, string[]> = {};
    for (const [name, size] of [
      ["desktop", DESKTOP],
      ["mobile", MOBILE],
    ] as const) {
      await page.setViewportSize(size);
      await open(page, "/admin/members", PAGES[0]?.ready as never);
      await page
        .getByRole("button", { name: /展开.*的详情/ })
        .first()
        .click();
      sizes[name] = await page.evaluate(() =>
        ["h1", ".mb-name", ".mb-verdict", ".mb-kv", ".mb-chip", ".btn"].map((sel) => {
          const el = document.querySelector(sel);
          return el ? getComputedStyle(el).fontSize : "none";
        }),
      );
    }
    expect(sizes.mobile).toEqual(sizes.desktop);
  });

  test("截图：浅色 / 深色 × 桌面 / 手机", async ({ page }) => {
    test.skip(!process.env.E2E_SHOTS, "设置 E2E_SHOTS=<目录> 才截图（截图不提交）");
    test.setTimeout(240_000);
    mkdirSync(SHOTS, { recursive: true });
    for (const scheme of ["light", "dark"] as const) {
      await page.emulateMedia({ colorScheme: scheme });
      for (const [vpName, size] of [
        ["desktop", DESKTOP],
        ["mobile", MOBILE],
      ] as const) {
        await page.setViewportSize(size);
        const tag = `${vpName}-${scheme}`;
        for (const p of PAGES) {
          await open(page, p.path, p.ready);
          await page.waitForTimeout(500);
          await page.screenshot({ path: `${SHOTS}/${p.name}-${tag}.png`, fullPage: true });
        }
        // 展开的成员 + Key 编辑框
        await open(page, "/admin/members", PAGES[0]?.ready as never);
        await page
          .getByRole("button", { name: /展开.*的详情/ })
          .first()
          .click();
        await page.waitForTimeout(300);
        await page.screenshot({ path: `${SHOTS}/members-expanded-${tag}.png`, fullPage: true });
        await page.getByRole("button", { name: "成员例外" }).click();
        await expect(page.getByRole("dialog", { name: /成员例外/ })).toBeVisible();
        await page.waitForTimeout(300);
        await page.screenshot({ path: `${SHOTS}/members-keydialog-${tag}.png` });
        await page.keyboard.press("Escape");
      }
    }
  });
});
