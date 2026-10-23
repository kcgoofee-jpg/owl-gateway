/**
 * 各个 spec 共用的工具：控制台报错收集、axe 无障碍检查、横向滚动检查、改写真实响应、成员 / 后台会话、接口路径。
 */
import AxeBuilder from "@axe-core/playwright";
import { type Browser, type BrowserContext, type Cookie, expect, type Page } from "@playwright/test";
import { readCreds } from "./devEnv";

export const LIVE = /\/public\/live(\?.*)?$/;
export const STATUS = /\/public\/status(\?.*)?$/;
export const ME = /\/public\/me(\?.*)?$/;

/**
 * 收集控制台错误和页面异常。ignoreUrl：来自这个地址的报错不算（实时推送断线时页面会自己退回轮询，
 * 首页传 /public/live/stream，后台传 /admin/api/events）。
 */
export function watchConsole(page: Page, ignoreUrl?: string): string[] {
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(`pageerror: ${e.message}`));
  page.on("console", (m) => {
    if (m.type() !== "error") return;
    if (ignoreUrl && m.location().url.includes(ignoreUrl)) return;
    errors.push(`${m.text()} @ ${m.location().url}`);
  });
  return errors;
}

/** axe 报出的 serious / critical 问题，每条一行；没有问题就是空数组。 */
export async function seriousViolations(page: Page): Promise<string[]> {
  const r = await new AxeBuilder({ page }).analyze();
  return r.violations
    .filter((v) => v.impact === "serious" || v.impact === "critical")
    .map((v) => `${v.id} (${v.impact}): ${v.nodes.map((n) => n.target.join(" ")).join(" | ")}`);
}

/**
 * 页面没有横向滚动条。默认 scrollWidth <= clientWidth；loose = 后台页的口径：和窗口宽度比，留 1px 容差。
 */
export const noHorizontalScroll = (page: Page, opts: { loose?: boolean } = {}) =>
  page.evaluate(
    (loose) =>
      loose
        ? document.documentElement.scrollWidth <= window.innerWidth + 1
        : document.documentElement.scrollWidth <= document.documentElement.clientWidth,
    opts.loose === true,
  );

/** 在真实响应上改字段（保持其余字段真实）。 */
export async function patch(
  page: Page,
  url: RegExp,
  fn: (body: Record<string, unknown>) => Record<string, unknown>,
) {
  await page.route(url, async (route) => {
    try {
      const res = await route.fetch();
      const body = (await res.json()) as Record<string, unknown>;
      await route.fulfill({ response: res, json: fn(body) });
    } catch {
      // 测试结束、页面已关闭时还在途的轮询：忽略
      await route.abort().catch(() => {});
    }
  });
}

/** 带上演示数据里的成员会话 Cookie（owl_member）。 */
export async function asMember(context: BrowserContext) {
  await context.addCookies([
    { name: "owl_member", value: readCreds().member ?? "", domain: "localhost", path: "/" },
  ]);
}

/**
 * 登录后台，返回会话 Cookie。后台限制登录尝试次数（5 次 / 5 分钟），所以每个 spec 只登录一次，
 * 把会话存下来各测试复用；没有密码时怎么办（抛错还是跳过）由调用方决定。
 */
export async function adminSession(
  browser: Browser,
  baseURL: string | undefined,
  password: string,
): Promise<Cookie[]> {
  const ctx = await browser.newContext({ ...(baseURL ? { baseURL } : {}) });
  const r = await ctx.request.post("/admin/api/login", { data: { password } });
  expect(r.ok(), "后台登录失败（可能触发了登录次数限制，等 5 分钟或调大 LOGIN_MAX_ATTEMPTS）").toBeTruthy();
  const cookies = await ctx.cookies();
  await ctx.close();
  return cookies;
}
