/**
 * 「点了日志再点别的栏目，会跳回日志页」的回归：各页是懒加载的，点别的栏目时旧的日志页还会带着新地址的 path 多渲染一阵，
 * 这时不能再把人 replace 回日志页。旧地址（生成记录搬走了）换到新分区。
 */
import { render } from "preact";
import { act } from "preact/test-utils";
import { afterEach, describe, expect, it, vi } from "vitest";

const loc = vi.hoisted(() => ({ path: "/admin/logs", route: vi.fn(), query: {} as Record<string, string> }));
vi.mock("preact-iso", async (orig) => ({
  ...(await orig<typeof import("preact-iso")>()),
  useLocation: () => loc,
}));
vi.mock("./logs/Actions", () => ({ Actions: () => <div>Actions</div> }));
vi.mock("./logs/Usage", () => ({ Usage: () => <div>Usage</div> }));

import Logs, { inLogs } from "./logs";

let host: HTMLElement | null = null;
async function mount(path: string) {
  loc.path = path;
  loc.route.mockClear();
  host = document.createElement("div");
  document.body.append(host);
  await act(() => {
    render(<Logs />, host as HTMLElement);
  });
}
afterEach(() => {
  if (host) render(null, host);
  host?.remove();
  host = null;
});

describe("inLogs", () => {
  it("只认 /admin/logs 和它之下的路径", () => {
    expect(inLogs("/admin/logs")).toBe(true);
    expect(inLogs("/admin/logs/usage")).toBe(true);
    expect(inLogs("/admin/logsx")).toBe(false);
    expect(inLogs("/admin/system")).toBe(false);
    expect(inLogs("/admin/members")).toBe(false);
  });
});

describe("日志页的默认子页跳转", () => {
  it("/admin/logs 和不认识的子路径：replace 到请求日志", async () => {
    await mount("/admin/logs");
    expect(loc.route).toHaveBeenCalledWith("/admin/logs/usage", true);
    await mount("/admin/logs/nope");
    expect(loc.route).toHaveBeenCalledWith("/admin/logs/usage", true);
  });

  it("已删除的提示词记录入口跳到请求日志", async () => {
    await mount("/admin/logs/prompts");
    expect(host?.textContent).not.toContain("提示词记录");
    expect(loc.route).toHaveBeenCalledWith("/admin/logs/usage", true);
  });

  it("已经点去别的栏目（地址变了、旧日志页还没卸载）：不能再跳回日志页", async () => {
    for (const p of ["/admin/system", "/admin/members", "/admin"]) {
      await mount(p);
      expect(loc.route, p).not.toHaveBeenCalled();
    }
  });

  it("认识的子页不跳", async () => {
    await mount("/admin/logs/usage");
    expect(loc.route).not.toHaveBeenCalled();
  });
});
