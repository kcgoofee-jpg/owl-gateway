import { render } from "preact";
import { act as renderAct } from "preact/test-utils";
import { afterEach, expect, it, vi } from "vitest";
import { act, useAdmin } from "../../data";
import { ShareUpstream } from "./share";

vi.mock("../../data", async (original) => ({
  ...(await original<object>()),
  act: vi.fn(),
  useAdmin: vi.fn(),
}));
let root: HTMLDivElement;
afterEach(() => {
  render(null, root);
  root.remove();
  vi.clearAllMocks();
});
async function show() {
  const account = {
    token_id: "share-a",
    position: 1,
    token: "NAI-…aaaa",
    image_host: "https://example.test",
    allow_anlas: false,
    admin_enabled: true,
    daily_anlas: 20,
    rpm: 3,
    concurrency: 1,
    post_wait_min: 1,
    post_wait_max: 2,
    v5_daily_limit: 100,
    anlas_used_today: 0,
  };
  vi.mocked(useAdmin).mockReturnValue({
    data: {
      accounts: [account, { ...account, token_id: "share-b", position: 2 }],
      fields: { v5_daily_limit: { min: 0, max: 9999 } },
      pool_size: 3,
    },
    error: null,
    loading: false,
  });
  vi.mocked(act).mockResolvedValue({ ok: true });
  root = document.createElement("div");
  document.body.append(root);
  await renderAct(() => render(<ShareUpstream />, root));
}
async function fill(label: string, value: string) {
  const field = [...root.querySelectorAll("form")][0];
  const lab = [...(field?.querySelectorAll("label") ?? [])].find((node) => node.textContent === label);
  const input = document.getElementById(lab?.htmlFor ?? "") as HTMLInputElement;
  await renderAct(() => {
    input.value = value;
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
}
it("saves the selected key's full policy without touching the other key", async () => {
  await show();
  await fill("出图 RPM", "8");
  await renderAct(() => root.querySelector<HTMLButtonElement>('[role="switch"]')?.click());
  await renderAct(async () => root.querySelector<HTMLButtonElement>("form .savebar button")?.click());
  expect(act).toHaveBeenCalledExactlyOnceWith(
    "PUT",
    "/admin/api/share-upstreams/share-a/settings",
    { allow_anlas: true, daily_anlas: 20, rpm: 8, concurrency: 1, post_wait_min: 1, post_wait_max: 2 },
    "拼车设置已保存",
  );
  expect(root.querySelectorAll('[role="switch"]')[1]?.getAttribute("aria-checked")).toBe("false");
});
it("blocks invalid ranges and keeps the draft after a failed save", async () => {
  await show();
  await fill("出图后最少等待", "5");
  expect(root.querySelector<HTMLButtonElement>("form .savebar button")?.disabled).toBe(true);
  expect(root.textContent).toContain("等待时间上限不能小于下限");
  await fill("出图后最多等待", "8");
  vi.mocked(act).mockResolvedValue(null);
  await renderAct(async () => root.querySelector<HTMLButtonElement>("form .savebar button")?.click());
  expect(root.querySelector<HTMLButtonElement>("form .savebar button")?.disabled).toBe(false);
  expect(root.textContent).toContain("有 2 项未保存");
});
