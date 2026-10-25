import { render } from "preact";
import { act } from "preact/test-utils";
import { afterEach, describe, expect, it, vi } from "vitest";

const mock = vi.hoisted(() => ({
  settings: {
    v5_access_mode: "restricted",
    v5_authorized_discord_ids: ["900000000000000001"],
    gateway_owner_discord_ids: [],
    gateway_admin_discord_ids: [],
  } as Record<string, unknown>,
}));
vi.mock("../Admin", async (orig) => ({
  ...(await orig<typeof import("../Admin")>()),
  useAdmin: (url: string) => ({
    data:
      url === "/admin/api/settings"
        ? mock.settings
        : {
            members: [
              { discord_id: "900000000000000001", name: "Alpha", discord: { display_name: "Alpha User" } },
              { discord_id: "900000000000000002", name: "Beta", discord: { display_name: "Beta User" } },
            ],
          },
    error: null,
    loading: false,
  }),
  useBusy: () => {},
  act: vi.fn(async (_method: string, _url: string, body: Record<string, unknown>) => {
    mock.settings = { ...mock.settings, ...body };
    return { ok: true };
  }),
}));

import { AccessPolicyCard } from "./limits";

let host: HTMLElement | null = null;
async function mount() {
  host ??= document.createElement("div");
  if (!host.isConnected) document.body.append(host);
  await act(async () => {
    render(<AccessPolicyCard />, host as HTMLElement);
    await Promise.resolve();
  });
}
afterEach(() => {
  if (host) render(null, host);
  host?.remove();
  host = null;
  mock.settings = {
    v5_access_mode: "restricted",
    v5_authorized_discord_ids: ["900000000000000001"],
    gateway_owner_discord_ids: [],
    gateway_admin_discord_ids: [],
  };
});

describe("V5 访问策略草稿", () => {
  it("后台刷新保留未保存选择；保存成功后接受服务端值", async () => {
    await mount();
    const select = host?.querySelector("select[multiple]") as HTMLSelectElement;
    const beta = select.options.item(1);
    if (!beta) throw new Error("missing Beta member option");
    beta.selected = true;
    select.dispatchEvent(new Event("change", { bubbles: true }));
    await act(() => Promise.resolve());
    expect(host?.textContent ?? "").toContain("Beta User");
    expect(
      [...(host?.querySelectorAll("button") ?? [])].find((b) => b.textContent?.includes("保存访问策略"))
        ?.disabled,
    ).toBe(false);

    mock.settings = { ...mock.settings };
    await mount(); // 模拟 SSE/15 秒 tick 后的新数据
    expect(host?.textContent ?? "").toContain("Beta User");

    const save = [...(host?.querySelectorAll("button") ?? [])].find((b) =>
      b.textContent?.includes("保存访问策略"),
    );
    await act(async () => {
      save?.click();
      await Promise.resolve();
    });
    await mount();
    expect(host?.textContent ?? "").toContain("Beta User");
    const latestSelect = host?.querySelector("select[multiple]") as HTMLSelectElement | null;
    expect(latestSelect?.options.item(1)?.selected).toBe(true);
  });
});
