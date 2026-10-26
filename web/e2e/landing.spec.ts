import { mkdirSync } from "node:fs";
import { expect, test } from "@playwright/test";
import { COOKIES_FILE, readCreds } from "./devEnv";
import {
  asMember,
  LIVE,
  ME,
  noHorizontalScroll,
  patch,
  STATUS,
  seriousViolations,
  watchConsole,
} from "./helpers";

/**
 * 首页端到端。对着 `cargo xtask dev` 的合成数据跑（见 playwright.config.ts 顶部的说明）；
 * 个别场景（冷却、重置券……）用 page.route 在真实响应上改字段来造。
 */

const member = readCreds();
const needMember = !member.member;

const LIVE_STREAM = "**/public/live/stream";

// 首页优先走 SSE（/public/live/stream），EventSource 收到非 200 才会放弃并退回轮询 /public/live。下面的用例都靠
// page.route 改轮询的响应来造场景，所以让 SSE 回 404（abort 的话 EventSource 会一直重连、不退回轮询）；
// 真实的 SSE 由「实时推送」那条用例单独验证。
test.beforeEach(async ({ page }) => {
  await page.route(LIVE_STREAM, (route) => route.fulfill({ status: 404, body: "" }));
});

test.describe("游客", () => {
  test("渲染：实时状态、客户端设置、使用须知、页脚", async ({ page }) => {
    const errors = watchConsole(page, "/public/live/stream");
    await page.goto("/");
    // h1 只给读屏：屏幕上不显示标题、介绍语和说明小字
    const h1 = page.getByRole("heading", { level: 1, name: "猫头鹰公益站" });
    await expect(h1).toHaveCount(1);
    await expect(h1).toHaveClass(/sr-only/);
    await expect(page.locator(".lp-lead")).toHaveCount(0);

    // 实时状态：一句话 + 两条用量；两个数字都是「剩余」，V5 不限量时写「不限」，没有进度条
    const status = page.getByRole("region", { name: "当前状态" });
    await expect(status).toBeVisible();
    await expect(status.locator(".lp-status__text")).not.toBeEmpty();
    await expect(status.locator(".lp-meter")).toHaveCount(2);
    await expect(status.getByRole("progressbar", { name: "V4.5" })).toBeVisible();
    await expect(status).not.toContainText("全站剩余");
    await expect(status).not.toContainText(/已用 \d+%/);
    await expect(status).not.toContainText("登录时会读取");
    await expect(page.getByRole("banner").getByRole("link", { name: "用 Discord 登录" })).toBeVisible();
    await expect(status.getByRole("link", { name: "用 Discord 登录" })).toHaveCount(0);
    await expect(status.locator(".lp-meter").nth(1)).toContainText("V5");
    await expect(page.getByRole("status").filter({ hasText: /\S/ })).not.toHaveCount(0);

    // 使用指南：两个客户端与使用须知共用标签，方向键可切换
    const tabs = page.getByRole("tablist", { name: "使用指南" }).getByRole("tab");
    await expect(tabs).toHaveText(["柏宝绘", "智绘姬", "使用须知"]);
    await tabs.first().focus();
    await page.keyboard.press("ArrowRight");
    await expect(tabs.nth(1)).toHaveAttribute("aria-selected", "true");
    await expect(page.getByRole("tabpanel")).toContainText("生图间隔时间");

    // 使用须知：数字来自接口，页面里不再有旧文案
    await tabs.nth(2).click();
    const rules = page.locator(".lp-rules");
    await expect(rules).toContainText("每日 7 点重置");
    await expect(rules).toContainText("每人每日上限");
    await expect(rules).toContainText("V5 可能因账号额度恢复暂停");
    // 使用须知里不写接入地址（地址只在客户端设置里出现）
    await expect(rules.getByText("地址", { exact: false })).toHaveCount(0);
    const text = await page.locator("main").innerText();
    for (const gone of [
      "3 小时",
      "证据",
      "候补",
      "缩略图",
      "闲置",
      "1000 点",
      "分享核对",
      "保底",
      "按算力折算",
      "每小时自动恢复",
      "OpenAI 兼容",
    ]) {
      expect(text, `旧文案「${gone}」应已删除`).not.toContain(gone);
    }

    // 页脚
    const foot = page.getByRole("contentinfo");
    await expect(foot).toContainText("非商业公益服务，不保证可用性");
    // 「本站说明」页已下线（站长 10-22）：页脚不再有这个入口
    await expect(foot.getByRole("link", { name: "本站说明" })).toHaveCount(0);
    await expect(foot.getByRole("link", { name: "隐私" })).toHaveCount(0);
    expect(errors).toEqual([]);
  });

  test("首屏数据：网关注入 owl-boot 时直接用，不再请求 /public/me", async ({ page }) => {
    const boot = {
      me: {
        logged_in: true,
        has_key: true,
        key: "nai-bootbootbootbootbootbootbootboot",
        discord: { display_name: "注入用户" },
        expires_at: null,
        today: {
          images: 3,
          v5: 1,
          legacy: 2,
          daily_images: 40,
          daily_v5: 5,
          anlas: 0,
          daily_anlas: 0,
          reset_hour: 7,
        },
        coupons: [],
      },
      live: {
        quota: { images_today: 123, images_cap: 500, v5_today: 9, v5_cap: 60 },
        queue: { waiting: 0, running: 0 },
        guard: { hour: 1, hour_cap: 100, cooldown: 0, blocked: null },
        cycle: 5,
      },
      status: { web_login: true },
    };
    await page.route("**/", async (route) => {
      const res = await route.fetch();
      const html = (await res.text()).replace(
        "<!--owl-boot-->",
        `<script type="application/json" id="owl-boot">${JSON.stringify(boot)}</script>`,
      );
      await route.fulfill({ response: res, body: html });
    });
    const meRequests: string[] = [];
    page.on("request", (r) => {
      if (/\/public\/me(\?|$)/.test(r.url())) meRequests.push(r.url());
    });
    // 首屏就绪前把真实的实时数据挡住，证明第一屏来自注入的数据
    await page.route(LIVE, (r) => r.abort());
    await page.goto("/");
    await expect(page.getByRole("banner").getByRole("button", { name: /注入用户/ })).toBeVisible();
    // 圆环里是今天剩余的数（40 − 2 = 38），下面写总量
    await expect(page.locator(".lp-quota")).toContainText("V4.5：今天还剩 38，每天 40");
    // 仪表写剩余张数：500 - 123 = 377，V5 60 - 9 = 51
    await expect(page.locator(".lp-status")).toContainText("V4.5 剩余377张");
    await expect(page.locator(".lp-status")).toContainText("V5 剩余51张");
    await expect(page.locator(".lp-status__text")).toHaveText("马上开始 · 通道空闲");
    expect(meRequests).toEqual([]);
  });

  test("网页登录开放时：整页只有顶栏一个登录入口，指向 /login", async ({ page }) => {
    await patch(page, STATUS, (b) => ({ ...b, web_login: true }));
    await patch(page, LIVE, (b) => ({ ...b, web_login: true }));
    await page.goto("/");
    const login = page.getByRole("link", { name: "用 Discord 登录" });
    await expect(login).toHaveCount(1);
    await expect(login.first()).toHaveAttribute("href", /^\/login(\?dev=[0-9a-f]{32})?$/);
    await expect(page.getByRole("banner").getByRole("link", { name: "用 Discord 登录" })).toBeVisible();
    await expect(page.locator(".lp-hero").getByRole("link", { name: "用 Discord 登录" })).toHaveCount(0);
    // 登录前不出现「加入 Discord」按钮
    await expect(page.getByRole("link", { name: "加入 Discord" })).toHaveCount(0);
  });

  test("顶栏：登录入口靠右显示；首页内容仍在同一个 760px 的列里", async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 900 });
    await patch(page, STATUS, (b) => ({ ...b, web_login: true }));
    await patch(page, LIVE, (b) => ({ ...b, web_login: true }));
    await page.goto("/");
    const login = page.getByRole("link", { name: "用 Discord 登录" });
    const topbar = page.getByRole("banner");
    const controls = topbar.locator(".site-top__right");
    const [lb, cb] = [await login.boundingBox(), await controls.boundingBox()];
    expect(lb && cb && lb.x >= cb.x && lb.x + lb.width <= cb.x + cb.width).toBe(true);
    expect(await login.evaluate((el) => getComputedStyle(el).backgroundColor)).toBe("rgb(91, 91, 214)");
    await expect(login).toHaveText("登录");
    await expect(page.locator(".lp-status__head")).not.toBeEmpty();
    // 状态和说明区左右对齐，宽 760 以内
    const edges = await page
      .locator("main .lp-status, main .card:visible")
      .evaluateAll((els) =>
        els.map((e) => [
          Math.round(e.getBoundingClientRect().left),
          Math.round(e.getBoundingClientRect().right),
        ]),
      );
    expect(edges.length).toBeGreaterThanOrEqual(2);
    expect(new Set(edges.map((e) => e.join(","))).size).toBe(1);
    const [l, r] = edges[0] as number[];
    expect((r ?? 0) - (l ?? 0)).toBeLessThanOrEqual(760);
  });

  test("主题切换：写入 owl-theme，刷新后保持", async ({ page }) => {
    await page.emulateMedia({ colorScheme: "light" });
    await page.goto("/");
    const html = page.locator("html");
    await page.getByRole("button", { name: "切换到深色主题" }).click();
    await expect(html).toHaveAttribute("data-theme", "dark");
    expect(await page.evaluate(() => localStorage.getItem("owl-theme"))).toBe("dark");
    await expect(page.getByRole("button", { name: "切换到浅色主题" })).toBeVisible();
    await page.reload();
    await expect(html).toHaveAttribute("data-theme", "dark");
    await page.getByRole("button", { name: "切换到浅色主题" }).click();
    await expect(html).toHaveAttribute("data-theme", "light");
  });

  test("?login=failed 提示一次并清掉地址栏参数", async ({ page }) => {
    await page.goto("/?login=failed");
    await expect(page.getByText("Discord 登录没有成功")).toBeVisible();
    await expect.poll(() => new URL(page.url()).search).toBe("");
  });

  test("?verify=ok|no|error 各提示一句并清掉地址栏参数", async ({ page }) => {
    const cases: [string, string][] = [
      ["ok", "身份组验证通过"],
      ["no", "没有检测到「指定身份组」"],
      ["error", "Discord 暂时无法核验"],
    ];
    for (const [v, text] of cases) {
      await page.goto(`/?verify=${v}`);
      await expect(page.getByText(text)).toBeVisible();
      await expect.poll(() => new URL(page.url()).search).toBe("");
    }
  });

  test("?verify=no 提示里带上站点设置的身份组名称", async ({ page }) => {
    const withNote = (b: Record<string, unknown>) => ({
      ...b,
      registration: { ...(b.registration as object), role_note: "类脑「已验证」身份组" },
    });
    await patch(page, STATUS, withNote);
    await patch(page, LIVE, withNote);
    await page.goto("/?verify=no");
    await expect(page.getByText("没有检测到「类脑「已验证」身份组」，暂时不能领取 Key。")).toBeVisible();
  });

  test("站长公告已删除：首页没有公告 iframe", async ({ page }) => {
    await page.goto("/");
    await page.locator(".lp-status__text").waitFor();
    await expect(page.locator("iframe")).toHaveCount(0);
  });

  test("捐赠入口和提交接口已移除", async ({ page }) => {
    await page.goto("/");
    await expect(page.getByRole("banner").getByRole("button", { name: "捐赠" })).toHaveCount(0);
    await expect(page.getByRole("dialog", { name: /捐赠/ })).toHaveCount(0);
    const result = await page.request.post("/public/donate", { data: { token: "synthetic" } });
    expect(result.status()).toBe(404);
  });

  test("状态一句话：上游冷却 / 每日上限，与 API 原因同一说法", async ({ page }) => {
    const sentence = page.locator(".lp-status__text");
    await page.goto("/");
    const setBlocked = async (guard: Record<string, unknown>) => {
      await page.unroute(LIVE).catch(() => {});
      await patch(page, LIVE, (b) => ({ ...b, guard: { ...(b.guard as object), ...guard } }));
      await page.reload();
    };
    await setBlocked({ cooldown: 45, blocked: null });
    await expect(sentence).toHaveText("NovelAI 限流中 · 暂停约 45 秒");

    await setBlocked({
      cooldown: 0,
      blocked: {
        code: "daily_cap",
        text: "本站今日出图总量已达上限，每个账号每天 2400 张，未扣费。明天 7 点恢复。",
      },
    });
    await expect(sentence).toContainText("今日出图已满");
    // 读屏播报区同步更新
    await expect(page.locator("[role=status][aria-live=polite].sr-only").first()).toContainText(
      "今日出图已满",
    );
  });

  test("实时推送：不拦截 SSE 时页面连上 /public/live/stream，并显示网关推来的状态", async ({ page }) => {
    await page.route(LIVE_STREAM, (route) => route.continue()); // 盖掉上面 beforeEach 的拦截
    const stream = page.waitForResponse(LIVE_STREAM);
    await page.goto("/");
    const res = await stream;
    expect(res.status()).toBe(200);
    expect(res.headers()["content-type"]).toContain("text/event-stream");
    await expect(page.locator(".lp-status__text")).not.toBeEmpty();
    await expect(page.locator(".lp-status").getByRole("progressbar", { name: "V4.5" })).toBeVisible();
  });

  test("手机 375px：无横向滚动、控件不溢出", async ({ page }) => {
    await page.setViewportSize({ width: 375, height: 812 });
    await patch(page, STATUS, (b) => ({ ...b, web_login: true }));
    await patch(page, LIVE, (b) => ({ ...b, web_login: true }));
    await page.goto("/");
    const login = page.getByRole("banner").getByRole("link", { name: "用 Discord 登录" });
    await expect(login).toBeVisible();
    expect(await noHorizontalScroll(page)).toBe(true);
    // 顶栏的几个控件都在视口内
    for (const el of [page.getByRole("button", { name: /切换到/ }), login]) {
      const box = await el.boundingBox();
      expect(box && box.x >= 0 && box.x + box.width <= 375).toBe(true);
    }
  });

  for (const scheme of ["light", "dark"] as const) {
    test(`无障碍（axe）：游客 ${scheme} 没有严重 / 致命问题`, async ({ page }) => {
      await page.emulateMedia({ colorScheme: scheme });
      await patch(page, STATUS, (b) => ({ ...b, web_login: true }));
      await patch(page, LIVE, (b) => ({ ...b, web_login: true }));
      await page.goto("/");
      await page.locator(".lp-status__text").waitFor();
      expect(await seriousViolations(page)).toEqual([]);
      // 切到另一个客户端标签后再测一遍
      await page.getByRole("tab", { name: /智绘姬/ }).click();
      expect(await seriousViolations(page)).toEqual([]);
    });
  }
});

test.describe("登录成员", () => {
  test.skip(needMember, `没有成员会话文件 ${COOKIES_FILE}（先跑 cargo xtask dev）`);
  test.beforeEach(async ({ context, page }) => {
    await asMember(context);
    const base = process.env.E2E_BASE ?? "http://localhost:5174";
    const survey = await context.request.post(new URL("/member/survey", base).toString(), {
      data: { client: "Playwright" },
    });
    expect(survey.ok()).toBe(true);
    await patch(page, ME, (b) => ({ ...b, survey_done: true }));
  });

  test("首屏是额度圆环：V4.5 / V5 / Anlas，环里是今天剩余，下面「类型 总量」；不显示「我今天的额度」", async ({
    page,
  }) => {
    const errors = watchConsole(page, "/public/live/stream");
    await page.goto("/");
    const quota = page.locator(".lp-quota");
    const box = await quota.boundingBox();
    expect(box && box.y < 300).toBe(true);
    await expect(quota).not.toContainText("点重置");
    await expect(quota.locator(".ring-item")).toHaveCount(3);
    await expect(quota.locator(".ring-label b")).toHaveText(["V4.5", "V5", "Anlas"]);
    // 环下面只有类型名（18px / 600，字号四档制的标题档），不再显示每天总量；类型名和圆环水平居中对齐
    const label = quota.locator(".ring-label").first();
    expect(
      await label
        .locator("b")
        .evaluate((el) => [getComputedStyle(el).fontSize, getComputedStyle(el).fontWeight]),
    ).toEqual(["18px", "600"]);
    await expect(quota.locator(".ring-label span")).toHaveCount(0);
    for (const item of await quota.locator(".ring-item").all()) {
      const [ring, name] = await Promise.all([
        item.locator(".ring").boundingBox(),
        item.locator(".ring-label b").boundingBox(),
      ]);
      const mid = (b: typeof ring) => (b ? b.x + b.width / 2 : 0);
      expect(Math.abs(mid(ring) - mid(name))).toBeLessThan(1.5);
    }
    // 环里的数 = 上限 − 今天已用
    for (const item of await quota.locator(".ring-item").all()) {
      const sr = (await item.locator(".sr-only").textContent()) ?? "";
      const m = /今天还剩 ([\d,]+)，每天 ([\d,]+)/.exec(sr);
      if (m) await expect(item.locator(".ring__num")).toHaveText(m[1] as string);
      else await expect(item.locator(".ring__num")).toHaveText("∞");
    }
    await expect(page.getByText("我今天的额度")).toHaveCount(0);
    // 有 Key 的成员不显示三步上手；游客才有的内容不出现
    await expect(page.getByRole("heading", { name: "使用须知" })).toHaveCount(0);
    expect(errors).toEqual([]);
  });

  test("不限额的环：一整圈淡色实线，环里是 ∞", async ({ page }) => {
    await patch(page, ME, (b) => ({
      ...b,
      today: { ...(b.today as object), daily_images: 0 },
    }));
    await page.goto("/");
    const v45 = page.locator(".ring-item.t-v45");
    await expect(v45.locator(".ring__num")).toHaveText("∞");
    await expect(v45.locator(".ring__arc--unlimited")).toHaveCount(1);
  });

  test("Key：收进右上角账户菜单，可从菜单复制 Key 和接入地址", async ({ page, context }) => {
    await context.grantPermissions(["clipboard-read", "clipboard-write"]);
    await page.goto("/");
    const account = page.getByRole("banner").getByRole("button", { name: /月见草/ });
    await account.click();
    const menu = page.getByRole("menu");
    await expect(menu.getByText("我的 Key")).toBeVisible();
    await expect(menu.locator(".copy__value").first()).toHaveText(member.member_key ?? "");
    await menu.getByRole("button", { name: "复制Key" }).click();
    expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(member.member_key);
    await expect(menu.getByRole("button", { name: "复制接入地址" })).toBeVisible();
    // 站长规定：任何位置都没有钥匙图标
    await expect(page.locator("svg.lucide-key, svg.lucide-key-round, svg.lucide-key-square")).toHaveCount(0);
  });

  test("用户菜单：显示名，退出登录后回到游客视图", async ({ page }) => {
    await page.goto("/");
    const menuButton = page.getByRole("banner").getByRole("button", { name: /月见草/ });
    await expect(menuButton).toBeVisible();
    await menuButton.click();
    await page.getByRole("menuitem", { name: /退出登录/ }).click();
    await expect(page.getByRole("heading", { level: 1, name: "猫头鹰公益站" })).toHaveCount(1);
    await expect(page.locator(".lp-quota")).toHaveCount(0);
  });

  test("重置券：只有标题；范围 + 备注；倒计时 hh:mm:ss 在「使用」按钮里；点一下直接刷新", async ({
    page,
  }) => {
    const now = Math.floor(Date.now() / 1000);
    let used = false;
    await patch(page, ME, (b) => ({
      ...b,
      coupons: used
        ? []
        : [
            {
              id: 7,
              kind: "reset",
              note: "补偿 10-13",
              scope: "v45,v5,anlas",
              created_at: now,
              expires_at: now + 3 * 86400,
            },
          ],
    }));
    await page.route("**/public/me/coupons/7/use", async (route) => {
      used = true;
      await route.fulfill({ json: { ok: true } });
    });
    await page.goto("/");
    const card = page.locator("section", { has: page.getByRole("heading", { name: "重置券" }) });
    await expect(card.locator(".card__sub")).toHaveCount(0);
    await expect(card).toContainText("重置 V4.5、V5、Anlas");
    await expect(card).toContainText("补偿 10-13");
    const use = card.getByRole("button", { name: /^使用/ });
    // 三天：小时可以超过 24
    await expect(use.locator(".lp-coupon__cd")).toHaveText(/^7[12]:\d\d:\d\d$/);
    await use.click();
    await expect(page.getByRole("dialog", { name: "使用重置券" })).toHaveCount(0);
    await expect(page.getByText("已重置，今天的 V4.5、V5、Anlas 额度已恢复")).toBeVisible();
    expect(used).toBe(true);
    await expect(page.getByRole("heading", { name: "重置券" })).toHaveCount(0);
  });

  test("未填写问卷的登录用户自动弹出问卷", async ({ page }) => {
    await patch(page, ME, (b) => ({ ...b, survey_done: false }));
    let body: Record<string, unknown> | null = null;
    await page.route("**/member/survey", async (route) => {
      body = route.request().postDataJSON();
      await route.fulfill({ json: { ok: true } });
    });
    await page.goto("/");
    const dialog = page.getByRole("dialog", { name: "使用体验问卷" });
    await expect(dialog).toBeVisible();
    await dialog.getByLabel(/你主要用哪个客户端/).fill("柏宝绘");
    await dialog.getByRole("button", { name: "提交问卷" }).click();
    await expect(dialog).toBeHidden();
    expect(body).toEqual({ client: "柏宝绘", improve: [], improve_other: "" });
    await expect(page.getByRole("contentinfo").getByRole("link", { name: "使用体验问卷" })).toHaveAttribute(
      "href",
      "/survey",
    );
    await expect(page.getByRole("banner").getByRole("button", { name: "捐赠" })).toHaveCount(0);
  });

  test("从问卷页点登录回来：首页把人送回 /survey（一次性）", async ({ page }) => {
    await page.addInitScript(() => {
      if (location.pathname === "/") sessionStorage.setItem("owl-next", "/survey");
    });
    await page.goto("/");
    await page.waitForURL("**/survey");
    await expect(page.getByRole("heading", { level: 1, name: "成员问卷" })).toBeVisible();
    expect(await page.evaluate(() => sessionStorage.getItem("owl-next"))).toBeNull();
  });

  test("已登录但没有 Key：只有加入服务器的入口；邀请链接是新窗口外链，注册关闭时按钮置灰", async ({
    page,
  }) => {
    await patch(page, ME, (b) => ({ ...b, has_key: false, key: undefined, today: undefined }));
    const meta = (open: boolean) => (b: Record<string, unknown>) => ({
      ...b,
      discord_invite: "https://discord.gg/example",
      registration: { configured: true, open },
    });
    await patch(page, LIVE, meta(true));
    await patch(page, STATUS, meta(true));
    await page.goto("/");
    await expect(page.getByRole("heading", { name: "加入 Discord 服务器" })).toBeVisible();
    await expect(page.getByRole("link", { name: "加入 Discord" })).toHaveAttribute(
      "href",
      "https://discord.gg/example",
    );
    await expect(page.getByRole("link", { name: "加入 Discord" })).toHaveAttribute("rel", /noopener/);
    await expect(page.getByRole("button", { name: "复制领取命令" })).toHaveCount(0);
    await expect(page.locator(".lp-quota")).toHaveCount(0);
    // 加入卡在状态卡之前，两张同宽
    const j = await page.locator(".lp-join").boundingBox();
    const c = await page.locator(".lp-status").boundingBox();
    expect(j && c && j.y < c.y && Math.abs(j.width - c.width) < 1).toBe(true);

    await page.unroute(LIVE);
    await page.unroute(STATUS);
    await patch(page, LIVE, meta(false));
    await patch(page, STATUS, meta(false));
    await page.reload();
    await expect(page.getByRole("button", { name: "关闭注册" })).toBeDisabled();
  });

  test("重置券：没写范围的券（默认）只重置 V4.5", async ({ page }) => {
    const now = Math.floor(Date.now() / 1000);
    await patch(page, ME, (b) => ({
      ...b,
      coupons: [
        { id: 8, kind: "reset", note: "", scope: "v45", created_at: now, expires_at: now + 2 * 86400 },
      ],
    }));
    await page.route("**/public/me/coupons/8/use", (route) => route.fulfill({ json: { ok: true } }));
    await page.goto("/");
    const card = page.locator("section", { has: page.getByRole("heading", { name: "重置券" }) });
    await expect(card.locator(".lp-coupon__body")).toHaveText("重置 V4.5");
    await card.getByRole("button", { name: /^使用/ }).click();
    await expect(page.getByRole("dialog", { name: "使用重置券" })).toHaveCount(0);
    await expect(page.getByText("已重置，今天的 V4.5 额度已恢复")).toBeVisible();
  });

  test("手机 375px：无横向滚动；额度圆环在状态卡之前", async ({ page }) => {
    await page.setViewportSize({ width: 375, height: 812 });
    await page.goto("/");
    await expect(page.locator(".lp-quota")).toBeVisible();
    expect(await noHorizontalScroll(page)).toBe(true);
    const q = await page.locator(".lp-quota").boundingBox();
    const s = await page.locator(".lp-status").boundingBox();
    expect(q && s && q.y < s.y).toBe(true);
  });

  for (const scheme of ["light", "dark"] as const) {
    test(`无障碍（axe）：成员 ${scheme} 没有严重 / 致命问题`, async ({ page }) => {
      await page.emulateMedia({ colorScheme: scheme });
      const now = Math.floor(Date.now() / 1000);
      await patch(page, ME, (b) => ({
        ...b,
        coupons: [{ id: 1, kind: "reset", note: "测试", expires_at: now + 86400 }],
      }));
      await page.goto("/");
      await page.locator(".lp-quota").waitFor();
      expect(await seriousViolations(page)).toEqual([]);
      await page.getByRole("button", { name: /^使用/ }).click();
      expect(await seriousViolations(page)).toEqual([]);
    });
  }
});

/* 开屏和动效：其余用例按「减少动态效果」跑（playwright.config.ts），这里打开动效单独测 */
test.describe("开屏", () => {
  test.use({ reducedMotion: "no-preference" });

  test("眼睛眨完飞回 Logo；每天只播一次；点任意处跳过", async ({ page }) => {
    await page.goto("/");
    const splash = page.locator(".splash");
    await expect(splash).toBeVisible();
    await expect(splash.locator(".splash__eye")).toHaveCount(2);
    await expect(splash.locator(".splash__title")).toHaveText(/Drawing\s*fantasy/);
    // 播完自己消失，Logo 的眼睛回来
    await expect(splash).toHaveCount(0, { timeout: 8000 });
    await expect(page.locator(".brand__eyes i").first()).toHaveCSS("opacity", "1");
    expect(await page.evaluate(() => localStorage.getItem("owl-splash"))).toBe(
      await page.evaluate(() => new Date().toLocaleDateString("sv-SE")),
    );
    // 同一天再来不播
    await page.reload();
    await expect(page.locator(".lp-status__text")).not.toBeEmpty();
    await expect(splash).toHaveCount(0);

    // 换一天：播，点一下就跳过
    await page.evaluate(() => localStorage.setItem("owl-splash", "2000-01-01"));
    await page.reload();
    await expect(splash).toBeVisible();
    await splash.click();
    await expect(splash).toHaveCount(0, { timeout: 1000 });
  });

  test("系统开了减少动态效果：不播开屏", async ({ page }) => {
    await page.emulateMedia({ reducedMotion: "reduce" });
    await page.goto("/");
    await expect(page.locator(".lp-status__text")).not.toBeEmpty();
    await expect(page.locator(".splash")).toHaveCount(0);
  });

  test("字体自托管：开屏标题用同源的 woff2 子集，不连 Google Fonts", async ({ page }) => {
    const external: string[] = [];
    page.on("request", (r) => {
      if (/fonts\.(googleapis|gstatic)\.com/.test(r.url())) external.push(r.url());
    });
    const font = page.waitForResponse((r) => /\.woff2(\?|$)/.test(r.url()));
    await page.goto("/");
    const res = await font;
    expect(new URL(res.url()).origin).toBe(new URL(page.url()).origin);
    expect(res.status()).toBe(200);
    expect(external).toEqual([]);
  });
});

/* 独立状态页（网关渲染）：和首页同一套顶栏、令牌、卡片 */
test.describe("状态页", () => {
  test("顶栏和首页一样；标题只给读屏；横幅 26px，订阅按钮在横幅右边；两个组件在同一张卡片里", async ({
    page,
  }) => {
    await page.setViewportSize({ width: 1280, height: 900 });
    await page.goto("/status");
    const banner = page.getByRole("banner");
    await expect(banner.getByRole("link", { name: "猫头鹰公益站" })).toHaveAttribute("href", "/");
    await expect(banner.getByRole("link", { name: "状态" })).toHaveCount(0);
    await expect(banner.getByRole("link", { name: "捐赠" })).toHaveCount(0);
    await expect(banner.getByRole("button", { name: /切换到/ })).toBeVisible();
    await expect(page.getByRole("heading", { level: 1, name: "服务状态" })).toHaveClass(/sr-only/);
    const strong = page.locator(".sp-banner strong");
    expect(await strong.evaluate((el) => getComputedStyle(el).fontSize)).toBe("26px");
    const sub = page.locator(".sp-banner .sp-sub");
    const [sb, tb] = [await sub.boundingBox(), await strong.boundingBox()];
    expect(sb && tb && sb.x > tb.x + tb.width).toBe(true);
    await expect(page.locator(".sp-comps .sp-comp")).toHaveCount(2);
    const strips = page.locator(".sp-comps svg");
    await expect(strips).toHaveCount(2);
    const dayCount = await strips.first().locator("title").first().textContent();
    const days = Number(dayCount?.match(/共 (\d+) 天/)?.[1]);
    expect(days).toBeGreaterThanOrEqual(1);
    await expect(page.locator(".sp-comps rect")).toHaveCount(days * 2);
    // 页面里没有内联 style（CSP 是 style-src 'self'）
    expect(await page.locator("[style]").count()).toBe(0);
  });

  test("手机 375 / 320px：横幅标题和订阅按钮同一行不重叠，组件名完整不截断", async ({ page }) => {
    for (const width of [375, 320]) {
      await page.setViewportSize({ width, height: 800 });
      await page.goto("/status");
      const strong = page.locator(".sp-banner strong");
      const sub = page.locator(".sp-banner .sp-sub");
      const [tb, sb] = [await strong.boundingBox(), await sub.boundingBox()];
      expect(tb && sb, `${width}px`).toBeTruthy();
      if (!tb || !sb) continue;
      expect(Math.abs(tb.y + tb.height / 2 - (sb.y + sb.height / 2)), `${width}px 同一行`).toBeLessThan(6);
      expect(tb.x + tb.width, `${width}px 不重叠`).toBeLessThanOrEqual(sb.x + 1);
      // 文字没有溢出自己的框（标题框会撑满到按钮边上，只看框的位置判断不了文字是否压到按钮）
      expect(
        await strong.evaluate((el) => el.scrollWidth <= el.clientWidth + 1),
        `${width}px 标题不溢出`,
      ).toBe(true);
      // 站长 10-24：状态页标题在窄屏允许折行（按钮文案变长），但不能溢出容器
      const lh = await strong.evaluate((el) => parseFloat(getComputedStyle(el).lineHeight));
      expect(tb.height, `${width}px 标题不超过三行`).toBeLessThan(lh * 3.5);
      // 组件名不被省略号截断
      for (const name of await page.locator(".sp-name").all()) {
        expect(
          await name.evaluate((el) => el.scrollWidth <= el.clientWidth + 1),
          `${width}px 组件名完整`,
        ).toBe(true);
      }
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    }
  });

  test("主题切换和首页共用 owl-theme", async ({ page }) => {
    await page.emulateMedia({ colorScheme: "light" });
    await page.goto("/status");
    await page.getByRole("button", { name: "切换到深色主题" }).click();
    await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
    expect(await page.evaluate(() => localStorage.getItem("owl-theme"))).toBe("dark");
    await expect(page.getByRole("button", { name: "切换到浅色主题" })).toBeVisible();
  });

  for (const scheme of ["light", "dark"] as const) {
    test(`无障碍（axe）：状态页 ${scheme} 没有严重 / 致命问题`, async ({ page }) => {
      await page.emulateMedia({ colorScheme: scheme });
      await page.goto("/status");
      await page.locator(".sp-banner").waitFor();
      expect(await seriousViolations(page)).toEqual([]);
    });
  }
});

/* 截图：E2E_SHOTS=/tmp/owl-shots-landing pnpm exec playwright test landing -g 截图 */
test.describe("截图", () => {
  test.skip(!process.env.E2E_SHOTS, "设置 E2E_SHOTS=<目录> 才截图（截图不提交）");
  const sizes = { desktop: { width: 1280, height: 900 }, mobile: { width: 375, height: 812 } };
  for (const who of ["guest", "member"] as const) {
    for (const [name, viewport] of Object.entries(sizes)) {
      for (const scheme of ["light", "dark"] as const) {
        test(`截图 ${who} ${name} ${scheme}`, async ({ page, context }) => {
          test.skip(who === "member" && needMember, "没有成员会话");
          const dir = process.env.E2E_SHOTS ?? "";
          mkdirSync(dir, { recursive: true });
          if (who === "member") await asMember(context);
          await page.setViewportSize(viewport);
          await page.emulateMedia({ colorScheme: scheme });
          if (who === "guest") {
            await patch(page, STATUS, (b) => ({
              ...b,
              web_login: true,
              discord_invite: "https://discord.gg/owl",
            }));
            await patch(page, LIVE, (b) => ({
              ...b,
              web_login: true,
              registration: { configured: true, open: true },
              discord_invite: "https://discord.gg/owl",
            }));
          } else {
            const now = Math.floor(Date.now() / 1000);
            await patch(page, ME, (b) => ({
              ...b,
              coupons: [{ id: 1, kind: "reset", note: "补偿 10-13", expires_at: now + 3 * 86400 }],
            }));
          }
          await page.goto("/");
          await page.locator(".lp-status__text").waitFor();
          await page.waitForTimeout(400);
          await page.screenshot({ path: `${dir}/landing-${who}-${name}-${scheme}.png`, fullPage: true });
        });
      }
    }
  }
});
