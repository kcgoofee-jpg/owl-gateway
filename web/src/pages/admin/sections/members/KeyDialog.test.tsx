import { render } from "preact";
import { act } from "preact/test-utils";
import { afterEach, expect, it, vi } from "vitest";
import type { MemberPolicy } from "../../../../lib/member-policy";
import type { PolicyBound } from "../member-policy";
import { call } from "./actions";
import { KeyDialog } from "./KeyDialog";
import type { MemberRow } from "./types";

vi.mock("./actions", () => ({ call: vi.fn() }));
const defaults: MemberPolicy = {
  daily_images: 100,
  daily_v5: 5,
  daily_anlas: 15,
  monthly_anlas: 100,
  rpm: 10,
  image: true,
  v5: true,
  vibe: true,
  tags: true,
  img2img: false,
  upscale: false,
  augment: false,
  paid_parameters: false,
};
const fields: Record<string, PolicyBound> = Object.fromEntries(
  Object.entries(defaults).map(([k, v]) => [
    k,
    typeof v === "boolean" ? { kind: "bool" } : { kind: "int", min: 0, max: 10000 },
  ]),
);
const member = {
  id: 1,
  name: "test",
  discord_id: "900000000000000001",
  discord: null,
  policy: { defaults, effective: { ...defaults }, overrides: { daily_v5: 5 } },
} as unknown as MemberRow;
let root: HTMLDivElement;
afterEach(() => {
  if (root) {
    render(null, root);
    root.remove();
  }
  vi.clearAllMocks();
});
it("keeps an existing equal-valued override while saving another change", async () => {
  vi.mocked(call).mockResolvedValue({ ok: true, data: {} });
  root = document.createElement("div");
  document.body.append(root);
  await act(() =>
    render(<KeyDialog target={member} fields={fields} onClose={() => {}} onSaved={() => {}} />, root),
  );
  expect(root.querySelector('[aria-label="恢复V5 每天默认"]')).not.toBeNull();
  const rpm = [...root.querySelectorAll("label")].find((x) => x.textContent?.includes("每分钟请求数"));
  const input = root.querySelector<HTMLInputElement>(`#${rpm?.htmlFor}`);
  await act(() => {
    if (input) {
      input.value = "20";
      input.dispatchEvent(new Event("input", { bubbles: true }));
    }
  });
  const form = root.querySelector("form");
  await act(async () => {
    form?.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
  });
  expect(call).toHaveBeenCalledWith("PUT", expect.stringContaining("/rules"), { daily_v5: 5, rpm: 20 });
});
it("can remove an equal-valued override without changing its displayed number", async () => {
  vi.mocked(call).mockResolvedValue({ ok: true, data: {} });
  root = document.createElement("div");
  document.body.append(root);
  await act(() =>
    render(<KeyDialog target={member} fields={fields} onClose={() => {}} onSaved={() => {}} />, root),
  );
  await act(() => (root.querySelector('[aria-label="恢复V5 每天默认"]') as HTMLButtonElement).click());
  await act(async () => {
    root.querySelector("form")?.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
  });
  expect(call).toHaveBeenCalledWith("PUT", expect.stringContaining("/rules"), { daily_v5: null });
});
