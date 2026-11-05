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
  const buttons = root.querySelectorAll("button");
  expect(buttons[1]?.disabled).toBe(true);
  await act(async () => {
    buttons[0]?.click();
  });
  expect(call).toHaveBeenCalledWith("PUT", "/admin/api/members/synthetic-subject/image-access", {
    enabled: true,
  });
  await act(() => render(<OwnerImageButtons subject="synthetic-subject" enabled />, root));
  expect(root.querySelectorAll("button")[0]?.disabled).toBe(true);
  await act(async () => {
    root.querySelectorAll("button")[1]?.click();
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
