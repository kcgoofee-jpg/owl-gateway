import { render } from "preact";
import { act } from "preact/test-utils";
import { expect, it, vi } from "vitest";
import { Availability } from "./MemberDetail";
import type { KeyRow, MemberRow } from "./types";

vi.mock("../../data", () => ({
  useAdmin: () => ({ data: { v5_paused: false }, error: null, loading: false }),
  refresh: vi.fn(),
}));
it("uses the selected key counter rather than the aggregate of a member's keys", () => {
  const m = {
    id: 1,
    all_key_ids: [1, 2],
    key_count: 2,
    today: { v5: 18 },
    share_check: "not_locked",
    paused_until: null,
  } as unknown as MemberRow;
  const k = {
    enabled: true,
    expires_at: null,
    used: { v5: 3 },
    policy: { effective: { image: true, v5: true, daily_v5: 10 } },
  } as unknown as KeyRow;
  const root = document.createElement("div");
  document.body.append(root);
  act(() => render(<Availability m={m} k={k} now={100} readAt={100} />, root));
  expect(root.textContent).toContain("3 张（当前 Key 的已确认用量");
  expect(root.textContent).not.toContain("18 张");
  expect(root.textContent).toContain("当前未被分享要求拦截");
  act(() => render(null, root));
  root.remove();
});
