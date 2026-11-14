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
it("separates donated shares from relay-platform shares by source", async () => {
  const base = {
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
      accounts: [
        { ...base, token_id: "relay-a", donated: false },
        { ...base, token_id: "donated-a", position: 3, token: "pk-C…zzzz", donated: true },
      ],
      fields: { v5_daily_limit: { min: 0, max: 9999 } },
      pool_size: 3,
    },
    error: null,
    loading: false,
  });
  root = document.createElement("div");
  document.body.append(root);

  // 拼车平台页：只列出未绑定的拼车，捐赠 Key 不挤在这里，且保留添加入口。
  await renderAct(() => render(<ShareUpstream source="relay" />, root));
  expect(root.textContent).toContain("拼车 1");
  expect(root.textContent).not.toContain("捐赠 3");
  expect(root.textContent).not.toContain("赞助席捐赠");
  expect(root.textContent).toContain("添加拼车网关");

  // 捐赠页：只列出捐赠号，标题「捐赠」+「赞助席捐赠」徽章，没有添加入口（捐赠来自席位绑定）。
  await renderAct(() => render(<ShareUpstream source="donated" />, root));
  expect(root.textContent).toContain("捐赠 3");
  expect(root.textContent).toContain("赞助席捐赠");
  expect(root.textContent).not.toContain("拼车 1");
  expect(root.textContent).not.toContain("添加拼车网关");
});
it("keeps recently saved settings in the next full save before the server refresh", async () => {
  await show();
  await fill("出图 RPM", "8");
  await renderAct(async () => root.querySelector<HTMLButtonElement>("form .savebar button")?.click());
  await renderAct(async () => {});
  await fill("出图并发", "2");
  await renderAct(async () => root.querySelector<HTMLButtonElement>("form .savebar button")?.click());
  expect(vi.mocked(act).mock.calls[1]?.[2]).toMatchObject({ rpm: 8, concurrency: 2 });
});
it("reports a partially saved two-request update", async () => {
  await show();
  await fill("免费 V5 日限", "90");
  await fill("出图 RPM", "8");
  vi.mocked(act).mockResolvedValueOnce({ ok: true }).mockResolvedValueOnce(null);
  await renderAct(async () => root.querySelector<HTMLButtonElement>("form .savebar button")?.click());
  await renderAct(async () => {});
  expect(root.textContent).toContain("免费 V5 日限已保存");
  expect(root.textContent).toContain("其他拼车设置的结果未确认");
});
