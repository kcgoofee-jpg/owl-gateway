import { render } from "preact";
import { act as flush } from "preact/test-utils";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { MemberPolicy } from "../../../lib/member-policy";

const store = vi.hoisted(
  () =>
    ({ rules: { defaults: {} }, runtime: {}, ops: {} }) as {
      rules: { defaults: MemberPolicy };
      runtime: Record<string, unknown>;
      ops: Record<string, unknown>;
    },
);
const write = vi.hoisted(() =>
  vi.fn(async (_method: string, url: string, body: Record<string, unknown>) => {
    if (url === "/admin/api/member-rules") store.rules = { defaults: body as MemberPolicy };
    if (url === "/admin/api/settings") store.runtime = body;
    return true;
  }),
);
vi.mock("../Admin", async (original) => ({
  ...(await original<typeof import("../Admin")>()),
  useAdmin: (url: string) => ({
    data:
      url === "/admin/api/member-rules"
        ? store.rules
        : url === "/admin/api/settings"
          ? store.runtime
          : url === "/admin/api/ops"
            ? store.ops
            : null,
    error: null,
    loading: false,
  }),
  act: write,
}));

import Limits from "./limits";
import Onboarding from "./onboarding";

let host: HTMLDivElement | null = null;
function reset() {
  store.rules = {
    defaults: {
      daily_images: 100,
      daily_v5: 5,
      daily_anlas: 15,
      monthly_anlas: 99999,
      rpm: 10,
      image: true,
      v5: false,
      vibe: true,
      tags: true,
      upscale: false,
      augment: false,
      img2img: false,
      paid_parameters: false,
    },
  };
  store.runtime = {
    budgets: { daily_v5: 150, daily_anlas: 300, monthly_anlas: 10000 },
    v5_paused: false,
    v5_medium_mode: "off",
  };
  store.ops = {
    registration: { configured: true, active: 1, open: true, max_users: 20, expires_days: 0, roles: [] },
    features: { global: { image: true }, labels: { image: "图片生成" } },
    log_retention: { origin_keep_days: 0 },
  };
}
async function mount(view: "rules" | "protection") {
  host ??= document.createElement("div");
  if (!host.isConnected) document.body.append(host);
  await flush(async () => {
    render(view === "rules" ? <Onboarding /> : <Limits />, host as HTMLDivElement);
    await Promise.resolve();
  });
}
function number(label: string): HTMLInputElement {
  const node = [...(host?.querySelectorAll("label") ?? [])].find((element) => element.textContent === label);
  const input = node ? document.getElementById(node.getAttribute("for") ?? "") : null;
  if (!(input instanceof HTMLInputElement)) throw new Error(`missing ${label}`);
  return input;
}
async function input(label: string, value: string) {
  await flush(() => {
    const node = number(label);
    node.value = value;
    node.dispatchEvent(new Event("input", { bubbles: true }));
  });
}
function save() {
  return [...(host?.querySelectorAll("button") ?? [])].find(
    (button) => button.textContent?.trim() === "保存",
  ) as HTMLButtonElement;
}
afterEach(() => {
  if (host) render(null, host);
  host?.remove();
  host = null;
  write.mockClear();
});
describe("规则与运行保护表单", () => {
  it("修改默认规则时刷新不覆盖草稿，保存完整数字与功能规则", async () => {
    reset();
    await mount("rules");
    await input("V4.5 免费图/天", "250");
    store.rules = { defaults: { ...store.rules.defaults, daily_images: 300 } };
    await mount("rules");
    expect(number("V4.5 免费图/天").value).toBe("250");
    await flush(async () => {
      save().click();
      await Promise.resolve();
    });
    expect(write).toHaveBeenCalledWith(
      "PUT",
      "/admin/api/member-rules",
      expect.objectContaining({ daily_images: 250, v5: false }),
      expect.any(String),
    );
    expect(store.rules.defaults.daily_images).toBe(250);
    for (const obsolete of ["自动", "继承", "号主身份", "站长身份", "兼容旧设置"])
      expect(host?.textContent).not.toContain(obsolete);
  });
  it("空值禁止保存，零是明确停用", async () => {
    reset();
    await mount("rules");
    await input("V4.5 免费图/天", "");
    expect(save().disabled).toBe(true);
    await input("V4.5 免费图/天", "0");
    expect(save().disabled).toBe(false);
    await flush(async () => {
      save().click();
      await Promise.resolve();
    });
    expect(store.rules.defaults.daily_images).toBe(0);
  });
  it("运行保护只提交共享预算和运行开关", async () => {
    reset();
    await mount("protection");
    await input("V5 全站每天", "0");
    await flush(async () => {
      save().click();
      await Promise.resolve();
    });
    expect(write).toHaveBeenCalledWith(
      "PUT",
      "/admin/api/settings",
      {
        budgets: { daily_v5: 0, daily_anlas: 300, monthly_anlas: 10000 },
        v5_paused: false,
        v5_medium_mode: "off",
      },
      expect.any(String),
    );
    expect(host?.textContent).not.toContain("V5 授权主体");
    expect(host?.textContent).not.toContain("继承");
  });
});
