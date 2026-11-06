/**
 * `release.spec.ts` 用的工具：成员 / 后台会话。
 *
 * 这里只留真正在用到的两个。以前还放着控制台报错收集、axe 无障碍检查、横向滚动检查、
 * 响应改写（`watchConsole` / `seriousViolations` / `noHorizontalScroll` / `patch` / `misalignedControls`）
 * ——它们唯一的调用方是那些 CI 从来不跑、已经跟着界面一起过时的 spec，2026-11-06 一并删掉。
 * 需要再加检查时按要测的东西重新写，别从旧文件里捞。
 */
import { type Browser, type BrowserContext, type Cookie, expect } from "@playwright/test";
import { readCreds } from "./devEnv";

/** 带上演示数据里的成员会话 Cookie（owl_member）。 */
export async function asMember(
  context: BrowserContext,
  base = process.env.E2E_BASE ?? "http://localhost:5174",
) {
  const host = new URL(base).hostname;
  await context.addCookies([
    { name: "owl_member", value: readCreds().member ?? "", domain: host, path: "/" },
  ]);
}

/**
 * 登录后台，返回会话 Cookie。后台限制登录尝试次数（5 次 / 5 分钟），所以用例只登录一次，
 * 把会话存下来复用；没有密码时怎么办由调用方决定。
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
