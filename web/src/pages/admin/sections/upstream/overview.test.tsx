import { render } from "preact";
import { act } from "preact/test-utils";
import { afterEach, expect, it, vi } from "vitest";
import { UpstreamOverview } from "./overview";

const state = vi.hoisted(() => ({ sharedError: false }));
vi.mock("../../data", () => ({
  useAdmin: (url: string) =>
    url.endsWith("share-upstreams")
      ? state.sharedError
        ? { data: null, loading: false, error: "失败" }
        : {
            loading: false,
            error: null,
            data: {
              accounts: [
                {
                  token_id: "s",
                  position: 1,
                  donated: true,
                  usable: true,
                  admin_enabled: true,
                  disabled: false,
                  v5_daily_limit: 0,
                  v5_used: 2,
                  allow_anlas: true,
                  daily_anlas: 0,
                  rpm: 0,
                  concurrency: 1,
                },
              ],
            },
          }
      : {
          loading: false,
          error: null,
          data: {
            generated_at: 100,
            settlements: [],
            csrf_token: "test",
            accounts: [
              {
                token_id: "o",
                position: 1,
                usable: false,
                admin_enabled: false,
                disabled: false,
                v5_daily_limit: 3,
                v5_used: 1,
                v5_held: 1,
                allow_anlas: false,
              },
            ],
          },
        },
}));
let root: HTMLDivElement;
afterEach(() => {
  render(null, root);
  root.remove();
  state.sharedError = false;
});
it("shows different sources, semantics and pending V5 without implying overall availability", () => {
  root = document.createElement("div");
  document.body.append(root);
  act(() => render(<UpstreamOverview />, root));
  expect(root.textContent).toContain("官方 #1");
  expect(root.textContent).toContain("捐赠 #1");
  expect(root.textContent).toContain("另有 1 个在途或未知预留");
  expect(root.textContent).toContain("每日 Anlas 上限：不限");
  expect(root.textContent).toContain("实际派发还受");
  expect(root.querySelector('a[href="/admin/upstream/donated"]')).not.toBeNull();
});
it("reports an unavailable source rather than presenting it as an empty pool", () => {
  state.sharedError = true;
  root = document.createElement("div");
  document.body.append(root);
  act(() => render(<UpstreamOverview />, root));
  expect(root.textContent).toContain("拼车与捐赠读取失败");
  expect(root.textContent).not.toContain("当前没有上游账号");
});
