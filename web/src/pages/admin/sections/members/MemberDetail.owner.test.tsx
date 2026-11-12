import { render } from "preact";
import { act } from "preact/test-utils";
import { afterEach, expect, it, vi } from "vitest";
import { call } from "./actions";
import { OwnerImageButtons } from "./MemberDetail";

vi.mock("./actions", () => ({ call: vi.fn() }));
let root: HTMLDivElement;
afterEach(() => {
  if (root) {
    render(null, root);
    root.remove();
  }
  vi.clearAllMocks();
});

it("switches the bound member using the existing admin API without changing credentials or usage", async () => {
  root = document.createElement("div");
  document.body.append(root);
  vi.mocked(call).mockResolvedValue({ ok: true, data: {} });
  await act(() => render(<OwnerImageButtons subject="synthetic-subject" enabled={false} />, root));
  expect([...root.querySelectorAll("button")].map((b) => b.textContent)).toEqual(["启用全部生图功能"]);
  await act(async () => {
    root.querySelector("button")?.click();
  });
  expect(call).toHaveBeenCalledWith("PUT", "/admin/api/members/synthetic-subject/image-access", {
    enabled: true,
  });
  await act(() => render(<OwnerImageButtons subject="synthetic-subject" enabled />, root));
  expect([...root.querySelectorAll("button")].map((b) => b.textContent)).toEqual(["恢复普通默认"]);
  await act(async () => {
    root.querySelector("button")?.click();
  });
  expect(call).toHaveBeenLastCalledWith("PUT", "/admin/api/members/synthetic-subject/image-access", {
    enabled: false,
  });
});

it("shows save errors and permits retry", async () => {
  root = document.createElement("div");
  document.body.append(root);
  vi.mocked(call).mockResolvedValue({ ok: false, error: "保存失败" });
  await act(() => render(<OwnerImageButtons subject="synthetic-subject" enabled={false} />, root));
  await act(async () => {
    root.querySelector("button")?.click();
  });
  expect(root.querySelector('[role="alert"]')?.textContent).toBe("保存失败");
  expect(root.querySelector("button")?.disabled).toBe(false);
});
