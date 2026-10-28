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

export async function misalignedControls(page: Page): Promise<string[]> {
  return page.evaluate(() => {
    const sel =
      "button:not(.switch):not(.tabs__tab):not(.lp-gallery__copy), select, input:not([type=checkbox]):not([type=radio]):not([type=hidden]), .xa-seg, .copy, .field-inline";
    // Native modal dialogs make the background inert; compare controls in the active layer.
    const root = document.querySelector("dialog[open]") ?? document;
    const els = [...root.querySelectorAll<HTMLElement>(sel)].filter((el) => {
      const r = el.getBoundingClientRect();
      if (!r.width || !r.height || getComputedStyle(el).visibility === "hidden") return false;
      // 复合控件只看外框
      if (el.parentElement?.closest(".xa-seg, .copy, .field-inline")) return false;
      // 收起的 <details> 里的控件看不见（Chrome 仍算出位置），标题行除外
      const d = el.closest("details:not([open])");
      if (d && !el.closest("summary")) return false;
      return !el.closest("table, nav, aside, [role=menu], dialog:not([open])");
    });
    const name = (el: HTMLElement) =>
      (el.getAttribute("aria-label") || el.textContent || el.className)
        .trim()
        .replace(/\s+/g, " ")
        .slice(0, 16);
    const out: string[] = [];
    const rs = els.map((el) => ({ el, r: el.getBoundingClientRect() }));
    for (let i = 0; i < rs.length; i++)
      for (let j = i + 1; j < rs.length; j++) {
        const a = rs[i] as (typeof rs)[number];
        const b = rs[j] as (typeof rs)[number];
        const overlap = Math.min(a.r.bottom, b.r.bottom) - Math.max(a.r.top, b.r.top);
        if (overlap < Math.min(a.r.height, b.r.height) * 0.4) continue;
        const gap = Math.max(a.r.left, b.r.left) - Math.min(a.r.right, b.r.right);
        if (gap > 48 || gap < -2) continue;
        if (Math.abs(a.r.height - b.r.height) > 1.5 || Math.abs(a.r.top - b.r.top) > 1.5)
          out.push(
            `「${name(a.el)}」${Math.round(a.r.height)}px vs 「${name(b.el)}」${Math.round(b.r.height)}px`,
          );
      }
    return out;
  });
}
