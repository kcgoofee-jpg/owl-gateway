/**
 * 来信拆成「问卷 / 文字反馈」两个子页后的地址回归：
 * - /admin/inbox（侧栏 / ⌘K 的分区根路径）replace 到问卷页；
 * - /admin/inbox/survey、/admin/inbox/feedback 都是认识的子页，不跳；
 * - 不认识的子页回问卷页；已经点去别的栏目时（懒加载，旧页还带着新地址的 path 多渲染一阵）不能再把人跳回来。
 * 做法与 logs.redirect.test.tsx 相同。
 */
import { render } from "preact";
import { act } from "preact/test-utils";
import { afterEach, describe, expect, it, vi } from "vitest";

const loc = vi.hoisted(() => ({ path: "/admin/inbox", route: vi.fn(), query: {} as Record<string, string> }));
vi.mock("preact-iso", async (orig) => ({
  ...(await orig<typeof import("preact-iso")>()),
  useLocation: () => loc,
}));
// 页面会取一次反馈条数做 tab 角标：测试里不要真的发请求
vi.mock("../../Admin", async (orig) => ({
  ...(await orig<typeof import("../../Admin")>()),
  useAdmin: () => ({ data: null, error: null, loading: false }),
}));
vi.mock("./Survey", () => ({ Survey: () => <div>Survey</div> }));
vi.mock("./Feedback", () => ({ Feedback: () => <div>Feedback</div> }));

import Inbox from "../inbox";

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

describe("来信的子页跳转", () => {
  it("/admin/inbox 和它下面不认识的子路径：replace 到问卷页", async () => {
    await mount("/admin/inbox");
    expect(loc.route).toHaveBeenCalledWith("/admin/inbox/survey", true);
    await mount("/admin/inbox/nope");
    expect(loc.route).toHaveBeenCalledWith("/admin/inbox/survey", true);
  });

  it("问卷 / 文字反馈是两个子页，不跳", async () => {
    await mount("/admin/inbox/survey");
    expect(loc.route).not.toHaveBeenCalled();
    await mount("/admin/inbox/feedback");
    expect(loc.route).not.toHaveBeenCalled();
  });

  it("已经点去别的栏目（地址变了、旧来信页还没卸载）：不能再跳", async () => {
    for (const p of ["/admin/system", "/admin/members", "/admin/logs/usage"]) {
      await mount(p);
      expect(loc.route, p).not.toHaveBeenCalled();
    }
  });
});
