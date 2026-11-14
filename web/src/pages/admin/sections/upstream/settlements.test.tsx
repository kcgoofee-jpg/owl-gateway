import { render } from "preact";
import { act } from "preact/test-utils";
import { expect, it, vi } from "vitest";
import { adminWrite } from "../../data";
import type { Settlement } from "./core";
import { Settlements } from "./settlements";

vi.mock("../../data", () => ({ adminWrite: vi.fn().mockResolvedValue({ ok: true }) }));
it("requires an explicit confirmation and a sufficiently detailed reason before releasing", async () => {
  const item: Settlement = {
    request_id: "test",
    rid: "abcd",
    key_id: 1,
    correlation_id: "",
    status: "uncertain",
    service_day: "2026-11-14",
    service_month: "2026-11",
    reserved_anlas: 2,
    reserved_v5: 1,
    reserved_legacy: 0,
    is_vibe: false,
  };
  const root = document.createElement("div");
  document.body.append(root);
  await act(() => render(<Settlements items={[item]} csrf="test-csrf" />, root));
  const button = root.querySelector("button");
  expect(button?.disabled).toBe(true);
  const checkbox = root.querySelector<HTMLInputElement>('input[type="checkbox"]');
  await act(() => checkbox?.click());
  expect(button?.disabled).toBe(true);
  const textarea = root.querySelector("textarea");
  await act(() => {
    if (textarea) {
      textarea.value = "已检查上游记录并确认该请求确实未执行";
      textarea.dispatchEvent(new Event("input", { bubbles: true }));
    }
  });
  expect(button?.disabled).toBe(false);
  await act(async () => button?.click());
  expect(adminWrite).toHaveBeenCalledWith(
    "POST",
    "/admin/api/request-settlements/test/release",
    { confirmed_not_executed: true, reason: "已检查上游记录并确认该请求确实未执行" },
    { notify: "return", headers: { "x-nai-admin-csrf": "test-csrf" } },
  );
  act(() => render(null, root));
  root.remove();
});
