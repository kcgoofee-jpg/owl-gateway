import { render } from "preact";
import { act } from "preact/test-utils";
import { afterEach, expect, it, vi } from "vitest";
import { call } from "./actions";
import { PatronSeatButtons } from "./MemberDetail";

vi.mock("./actions", () => ({ call: vi.fn() }));
vi.mock("../../data", () => ({
  useAdmin: () => ({
    data: {
      accounts: [{ token_id: "token-donated", position: 1, token: "pst-te…0001", exclusive: false }],
    },
    error: null,
    loading: false,
  }),
}));

let root: HTMLDivElement;
afterEach(() => {
  if (root) {
    render(null, root);
    root.remove();
  }
  vi.clearAllMocks();
});

it("opens a patron seat with queue jump and optional bound token", async () => {
  root = document.createElement("div");
  document.body.append(root);
  vi.mocked(call).mockResolvedValue({ ok: true, data: { portal_password: "once-only-password" } });
  await act(() => render(<PatronSeatButtons subject="900000000000000001" />, root));
  expect(root.textContent).not.toContain("插队");
  const enable = [...root.querySelectorAll("button")].find((b) => b.textContent === "开启");
  expect([...root.querySelectorAll("button")].map((b) => b.textContent)).toEqual(["开启"]);
  await act(async () => {
    enable?.click();
  });
  expect(call).toHaveBeenCalledWith("PUT", "/admin/api/members/900000000000000001/patron", {
    enabled: true,
    queue_jump: true,
    bound_token_id: "",
  });
  expect(root.textContent).toContain("once-only-password");
});

it("shows save errors", async () => {
  root = document.createElement("div");
  document.body.append(root);
  vi.mocked(call).mockResolvedValue({ ok: false, error: "这把上游 Token 已绑定其他赞助席" });
  await act(() =>
    render(
      <PatronSeatButtons
        subject="900000000000000001"
        patron={{
          enabled: true,
          queue_jump: true,
          self_service: true,
          has_password: true,
          bound_token_id: "token-donated",
        }}
      />,
      root,
    ),
  );
  expect(root.textContent).toContain("插队");
  expect([...root.querySelectorAll("button")].map((b) => b.textContent)).toEqual([
    "保存",
    "重置密码",
    "关闭",
  ]);
  const save = [...root.querySelectorAll("button")].find((b) => b.textContent === "保存");
  await act(async () => {
    save?.click();
  });
  expect(root.querySelector('[role="alert"]')?.textContent).toBe("这把上游 Token 已绑定其他赞助席");
});
