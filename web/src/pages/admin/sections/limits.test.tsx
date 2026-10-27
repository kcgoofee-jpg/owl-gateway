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

/** 「V5 授权主体」搜索框：聚焦打开下拉，点一个成员行加入 */
async function pickBeta() {
  const input = host?.querySelector<HTMLInputElement>(".ss__input");
  if (!input) throw new Error("missing member search input");
  await act(() => {
    input.focus();
  });
  const option = [...(host?.querySelectorAll('[role="option"]') ?? [])].find((o) =>
    o.textContent?.includes("Beta User"),
  );
  if (!option) throw new Error("missing Beta member option");
  await act(() => {
    option.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, cancelable: true }));
  });
}

const saveButton = () =>
  [...(host?.querySelectorAll("button") ?? [])].find((b) => b.textContent?.includes("保存访问策略"));

describe("V5 访问策略草稿", () => {
  it("后台刷新保留未保存选择；保存成功后接受服务端值", async () => {
    await mount();
    await pickBeta();
    expect(host?.textContent ?? "").toContain("Beta User");
    expect(saveButton()?.disabled).toBe(false);

    mock.settings = { ...mock.settings };
    await mount(); // 模拟 SSE/15 秒 tick 后的新数据
    expect(host?.textContent ?? "").toContain("Beta User");

    await act(async () => {
      saveButton()?.click();
      await Promise.resolve();
    });
    await mount();
    expect(host?.textContent ?? "").toContain("Beta User");
    expect(mock.settings.v5_authorized_discord_ids).toEqual(["900000000000000001", "900000000000000002"]);
  });
});
