/**
 * 来信并成一页后的旧地址回归：/admin/inbox 与旧的反馈子页（/admin/inbox/feedback）都 replace 到问卷页，查询串照带；
 * 点去别的栏目时（懒加载，旧页还带着新地址的 path 多渲染一阵）不能再把人跳回来。做法与 logs.redirect.test.tsx 相同。
 */
import { render } from "preact";
import { act } from "preact/test-utils";
import { afterEach, describe, expect, it, vi } from "vitest";

const loc = vi.hoisted(() => ({ path: "/admin/inbox", route: vi.fn(), query: {} as Record<string, string> }));
vi.mock("preact-iso", async (orig) => ({
  ...(await orig<typeof import("preact-iso")>()),
  useLocation: () => loc,
}));
vi.mock("./Survey", () => ({ Survey: () => <div>Survey</div> }));
vi.mock("./Feedback", () => ({ Feedback: () => <div>Feedback</div> }));

import Inbox, { inInbox } from "../inbox";

let host: HTMLElement | null = null;
async function mount(path: string) {
  loc.path = path;
  loc.route.mockClear();
  host = document.createElement("div");
  document.body.append(host);
  await act(() => {
    render(<Inbox />, host as HTMLElement);
  });
}
afterEach(() => {
  if (host) render(null, host);
  host?.remove();
  host = null;
});

describe("inInbox", () => {
  it("只认 /admin/inbox 和它之下的路径", () => {
    expect(inInbox("/admin/inbox")).toBe(true);
    expect(inInbox("/admin/inbox/survey")).toBe(true);
    expect(inInbox("/admin/inboxx")).toBe(false);
    expect(inInbox("/admin/system")).toBe(false);
    expect(inInbox("/admin/members")).toBe(false);
  });
});

describe("来信页的旧地址跳转", () => {
  it("/admin/inbox 和旧的反馈子页：replace 到问卷，查询串照带", async () => {
    await mount("/admin/inbox");
    expect(loc.route).toHaveBeenCalledWith("/admin/inbox/survey", true);
    await mount("/admin/inbox/feedback");
    expect(loc.route).toHaveBeenCalledWith("/admin/inbox/survey", true);
    loc.query = { key: "3,5" };
    await mount("/admin/inbox/feedback");
    expect(loc.route).toHaveBeenCalledWith("/admin/inbox/survey?key=3%2C5", true);
    loc.query = {};
  });

  it("问卷页不跳", async () => {
    await mount("/admin/inbox/survey");
    expect(loc.route).not.toHaveBeenCalled();
  });

  it("已经点去别的栏目（地址变了、旧来信页还没卸载）：不能再跳", async () => {
    for (const p of ["/admin/system", "/admin/members", "/admin/logs/usage"]) {
      await mount(p);
      expect(loc.route, p).not.toHaveBeenCalled();
    }
  });
});
