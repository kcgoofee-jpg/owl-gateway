import { render } from "preact";
import { act } from "preact/test-utils";
import { afterEach, expect, it, vi } from "vitest";
import { type DetailActions, MemberDetail } from "./MemberDetail";
import type { MemberRow } from "./types";

const state = vi.hoisted(() => ({ loading: true, error: null as string | null, refresh: vi.fn() }));
vi.mock("../../data", () => ({
  useAdmin: () => ({ data: null, error: state.error, loading: state.loading }),
  refresh: state.refresh,
}));
const member = {
  id: 1,
  name: "test",
  key_count: 1,
  discord: null,
  discord_id: null,
  tags: [],
  issues: [],
  enabled: true,
  expires_at: null,
  paused_until: null,
} as unknown as MemberRow;
const actions = {} as DetailActions;
let root: HTMLDivElement;
afterEach(() => {
  if (root) {
    render(null, root);
    root.remove();
  }
  state.loading = true;
  state.error = null;
  state.refresh.mockClear();
});
it("distinguishes loading from failure and offers retry", async () => {
  root = document.createElement("div");
  document.body.append(root);
  await act(() => render(<MemberDetail m={member} now={1} act={actions} />, root));
  expect(root.textContent).toContain("正在读取 Key 详情");
  expect(root.textContent).not.toContain("读取失败");
  state.loading = false;
  state.error = "network";
  await act(() => render(<MemberDetail m={{ ...member }} now={1} act={actions} />, root));
  expect(root.textContent).toContain("读取失败");
  await act(() => root.querySelector("button")?.click());
  expect(state.refresh).toHaveBeenCalledOnce();
});
it("shows an empty state when no key exists", async () => {
  root = document.createElement("div");
  document.body.append(root);
  await act(() => render(<MemberDetail m={{ ...member, key_count: 0 }} now={1} act={actions} />, root));
  expect(root.textContent).toContain("该成员没有 Key");
});
