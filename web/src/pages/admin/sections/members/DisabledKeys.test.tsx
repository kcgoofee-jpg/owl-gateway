import { render } from "preact";
import { act } from "preact/test-utils";
import { afterEach, expect, it, vi } from "vitest";
import { call } from "./actions";
import { DisabledKeysDialog } from "./Dialogs";

vi.mock("../../data", () => ({
  busy: { value: 0 },
  useAdmin: () => ({
    data: {
      keys: [
        {
          id: 42,
          name: "synthetic",
          discord_id: null,
          expires_at: null,
          disabled_reason: "admin",
          banned: 0,
        },
        {
          id: 43,
          name: "blocked",
          discord_id: "900000000000000001",
          expires_at: null,
          disabled_reason: "admin",
          banned: 1,
        },
      ],
    },
    error: null,
    loading: false,
  }),
}));
vi.mock("./actions", () => ({ call: vi.fn() }));
let root: HTMLDivElement;
afterEach(() => {
  if (root) {
    render(null, root);
    root.remove();
  }
  vi.clearAllMocks();
});
it("confirms restoration for the selected key and surfaces conflicts", async () => {
  vi.mocked(call).mockResolvedValue({ ok: false, error: "状态已变更" });
  root = document.createElement("div");
  document.body.append(root);
  await act(() => render(<DisabledKeysDialog onClose={() => {}} onChanged={() => {}} />, root));
  expect(root.textContent).toContain("synthetic");
  expect(root.textContent).toContain("已封禁");
  const blockedRow = [...root.querySelectorAll("tr")].find((row) => row.textContent?.includes("blocked"));
  expect(blockedRow?.querySelector<HTMLButtonElement>("button")?.disabled).toBe(true);
  await act(() => [...root.querySelectorAll("button")].find((b) => b.textContent === "恢复")?.click());
  const buttons = [...root.querySelectorAll("button")].filter((b) => b.textContent === "恢复");
  await act(async () => {
    buttons.at(-1)?.click();
  });
  expect(call).toHaveBeenCalledWith("PATCH", "/admin/api/keys/42", { enabled: true });
  await act(async () => {});
  expect(root.textContent).toContain("状态已变更");
});
