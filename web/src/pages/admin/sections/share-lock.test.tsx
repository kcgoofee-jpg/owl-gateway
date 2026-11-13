import { render } from "preact";
import { act as renderAct } from "preact/test-utils";
import { afterEach, expect, it, vi } from "vitest";
import { act, useAdmin } from "../data";
import { ShareLockCard, type ShareLockStatus } from "./share-lock";

vi.mock("../data", () => ({ act: vi.fn(), useAdmin: vi.fn() }));
let root: HTMLDivElement;
const status = (): ShareLockStatus => ({
  enabled: true,
  mode: "quota",
  enforcing: true,
  healthy: true,
  bot_online: true,
  channel_url: "https://discord.com/channels/900000000000000001/900000000000000002",
  detection: {
    connected: true,
    last_complete_at: 1800000000,
    complete_day_start: 1799967600,
    pending: 0,
    oldest_failure_at: null,
    error: null,
  },
});
afterEach(() => {
  if (root) {
    render(null, root);
    root.remove();
  }
  vi.clearAllMocks();
});
async function show(data: ShareLockStatus) {
  vi.mocked(useAdmin).mockReturnValue({ data, loading: false, error: "" });
  root = document.createElement("div");
  document.body.append(root);
  await renderAct(() => render(<ShareLockCard />, root));
}
it("persists the switch through the API and renders the confirmed server state", async () => {
  await show(status());
  vi.mocked(act).mockResolvedValue({ enabled: false });
  await renderAct(async () => {
    root.querySelector<HTMLButtonElement>('[role="switch"]')?.click();
  });
  expect(act).toHaveBeenCalledWith("PUT", "/admin/api/ops/share-lock", { enabled: false }, "分享限制已保存");
  vi.mocked(useAdmin).mockReturnValue({
    data: { ...status(), enabled: false, enforcing: false },
    loading: false,
    error: "",
  });
  await renderAct(() => render(<ShareLockCard />, root));
  expect(root.querySelector('[role="switch"]')?.getAttribute("aria-checked")).toBe("false");
  expect(root.textContent).toContain("分享限制已关闭，检测继续运行");
});
it("retains the saved switch value and allows retry on failure", async () => {
  await show(status());
  vi.mocked(act).mockResolvedValue(null);
  await renderAct(async () => {
    root.querySelector<HTMLButtonElement>('[role="switch"]')?.click();
  });
  expect(root.querySelector('[role="alert"]')?.textContent).toContain("保存失败");
  expect(root.querySelector('[role="switch"]')?.getAttribute("aria-checked")).toBe("true");
  expect(root.querySelector<HTMLButtonElement>('[role="switch"]')?.disabled).toBe(false);
});
it("distinguishes an online bot from a broken detector and explains fail-open", async () => {
  const data = status();
  data.healthy = false;
  data.enforcing = false;
  data.detection.pending = 2;
  data.detection.error = "读取历史权限不足";
  await show(data);
  expect(root.textContent).toContain("Bot 在线");
  expect(root.textContent).toContain("分享检测待恢复");
  expect(root.textContent).toContain("现暂时放行");
  expect(root.textContent).toContain("读取历史权限不足");
});

it("saves the independent daily mode and explains that quota exhaustion is not required", async () => {
  await show({ ...status(), mode: "daily" });
  expect(root.textContent).toContain("所有成员昨日或今日需分享一张图");
  vi.mocked(act).mockResolvedValue({ mode: "quota" });
  await renderAct(async () => {
    const select = root.querySelector<HTMLSelectElement>("#share-lock-mode");
    if (!select) throw new Error("mode selector missing");
    select.value = "quota";
    select.dispatchEvent(new Event("change", { bubbles: true }));
  });
  expect(act).toHaveBeenCalledWith("PUT", "/admin/api/ops/share-lock", { mode: "quota" }, "分享限制已保存");
});
