import { render } from "preact";
import { act as renderAct } from "preact/test-utils";
import { afterEach, expect, it, vi } from "vitest";
import { act } from "../../data";
import { ClaimForm } from "../settings";
import type { Ops } from "./core";

vi.mock("../../data", () => ({ act: vi.fn(), busy: { value: 0 }, useAdmin: vi.fn() }));
const ops: Ops = {
  registration: {
    configured: true,
    active: 0,
    open: true,
    max_users: 10,
    expires_days: 0,
    roles: [],
    fields: {
      max_users: { kind: "int", min: 0, max: 100 },
      expires_days: { kind: "int", min: 0, max: 100 },
      roles: { kind: "int", min: 0, max: 5 },
    },
  },
  features: { global: {}, labels: {} },
  log_retention: { origin_keep_days: 0 },
};
let root: HTMLDivElement;
afterEach(() => {
  if (root) {
    render(null, root);
    root.remove();
  }
  vi.clearAllMocks();
});
it("preserves a saved switch when a second field is saved before refresh", async () => {
  vi.mocked(act).mockResolvedValue({ ok: true });
  root = document.createElement("div");
  document.body.append(root);
  await renderAct(() => render(<ClaimForm ops={ops} />, root));
  await renderAct(() => root.querySelector<HTMLButtonElement>('[role="switch"]')?.click());
  await renderAct(async () => {
    root.querySelector("form")?.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
  });
  await renderAct(async () => {});
  expect(root.querySelector('[role="switch"]')?.getAttribute("aria-checked")).toBe("false");
  const label = [...root.querySelectorAll("label")].find((el) => el.textContent?.includes("名额上限"));
  const input = root.querySelector<HTMLInputElement>(`#${label?.htmlFor}`);
  await renderAct(() => {
    if (input) {
      input.value = "20";
      input.dispatchEvent(new Event("input", { bubbles: true }));
    }
  });
  await renderAct(async () => {
    root.querySelector("form")?.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
  });
  expect(vi.mocked(act).mock.calls[1]?.[2]).toMatchObject({ open: false, max_users: 20 });
});
