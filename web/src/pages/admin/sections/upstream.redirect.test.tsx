/** 「点了上游再点别的栏目，会跳回上游页」的回归：和日志页同一套 TabPage 判断。 */
import { render } from "preact";
import { act } from "preact/test-utils";
import { afterEach, describe, expect, it, vi } from "vitest";

const loc = vi.hoisted(() => ({
  path: "/admin/upstream",
  route: vi.fn(),
  query: {} as Record<string, string>,
}));
vi.mock("preact-iso", async (orig) => ({
  ...(await orig<typeof import("preact-iso")>()),
  useLocation: () => loc,
}));
vi.mock("./upstream/official", () => ({ OfficialUpstream: () => <div>Official</div> }));
vi.mock("./upstream/share", () => ({ ShareUpstream: () => <div>Share</div> }));

import Upstream, { inUpstream } from "./upstream";

let host: HTMLElement | null = null;
async function mount(path: string) {
  loc.path = path;
  loc.route.mockClear();
  host = document.createElement("div");
  document.body.append(host);
  await act(() => {
    render(<Upstream />, host as HTMLElement);
  });
}
afterEach(() => {
  if (host) render(null, host);
  host?.remove();
  host = null;
});

describe("inUpstream", () => {
  it("只认 /admin/upstream 和它之下的路径", () => {
    expect(inUpstream("/admin/upstream")).toBe(true);
    expect(inUpstream("/admin/upstream/official")).toBe(true);
    expect(inUpstream("/admin/upstreamx")).toBe(false);
    expect(inUpstream("/admin/logs")).toBe(false);
  });
});

describe("上游页的默认子页跳转", () => {
  it("/admin/upstream 和不认识的子路径：replace 到官方账号", async () => {
    await mount("/admin/upstream");
    expect(loc.route).toHaveBeenCalledWith("/admin/upstream/official", true);
    await mount("/admin/upstream/nope");
    expect(loc.route).toHaveBeenCalledWith("/admin/upstream/official", true);
  });

  it("已经点去别的栏目：不能再跳回上游页", async () => {
    for (const p of ["/admin/system", "/admin/members", "/admin"]) {
      await mount(p);
      expect(loc.route).not.toHaveBeenCalled();
    }
  });
});
