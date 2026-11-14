import { render } from "preact";
import { act as flush } from "preact/test-utils";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { MemberPolicy } from "../../../lib/member-policy";
import type { PolicyBound } from "./member-policy";

const RULE_FIELDS: Record<string, PolicyBound> = {
  daily_images: { kind: "int", min: 0, max: 9999 },
  daily_v5: { kind: "int", min: 0, max: 9999 },
  daily_anlas: { kind: "int", min: 0, max: 9999 },
  monthly_anlas: { kind: "float", min: 0, max: 99999 },
  rpm: { kind: "int", min: 1, max: 600 },
  image: { kind: "bool" },
  v5: { kind: "bool" },
  vibe: { kind: "bool" },
  tags: { kind: "bool" },
  img2img: { kind: "bool" },
  upscale: { kind: "bool" },
  augment: { kind: "bool" },
  paid_parameters: { kind: "bool" },
};

const store = vi.hoisted(
  () =>
    ({ rules: { defaults: {}, fields: {} }, runtime: {}, ops: {} }) as {
      rules: { defaults: MemberPolicy; fields: Record<string, PolicyBound> };
      runtime: Record<string, unknown>;
      ops: Record<string, unknown>;
    },
);
const write = vi.hoisted(() =>
  vi.fn(async (_method: string, url: string, body: Record<string, unknown>) => {
    if (url === "/admin/api/member-rules")
      store.rules = { defaults: body as MemberPolicy, fields: store.rules.fields };
    if (url === "/admin/api/settings") store.runtime = body;
    return true;
  }),
);
vi.mock("../data", async (original) => ({
  ...(await original<typeof import("../data")>()),
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

import Settings from "./settings";

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
      share_exempt: false,
    },
    fields: RULE_FIELDS,
  };
  store.runtime = {
    budgets: { daily_v5: 150, daily_anlas: 300, monthly_anlas: 10000 },
    v5_paused: false,
    v5_medium_mode: "off",
    fields: {
      daily_v5: { kind: "int", min: 0, max: 9999 },
      daily_anlas: { kind: "int", min: 0, max: 9999 },
      monthly_anlas: { kind: "float", min: 0, max: 99999 },
    },
  };
  store.ops = {
    maintenance: { enabled: false, mode: "off" },
    registration: {
      configured: true,
      active: 1,
      open: true,
      max_users: 20,
      expires_days: 9999,
      roles: [],
      fields: {
        max_users: { kind: "int", min: 0, max: 100000 },
        expires_days: { kind: "int", min: 0, max: 9999 },
        roles: { kind: "int", min: 0, max: 6 },
      },
    },
  };
}
async function mount() {
  host ??= document.createElement("div");
  if (!host.isConnected) document.body.append(host);
  await flush(async () => {
    render(<Settings />, host as HTMLDivElement);
    await Promise.resolve();
  });
}
/** 按卡片标题找输入框，避免不同卡片里同名字段串味。 */
function card(title: string): HTMLElement {
  const found = [...(host?.querySelectorAll<HTMLElement>(".card") ?? [])].find((element) =>
    element.querySelector(".card__title")?.textContent?.includes(title),
  );
  if (!found) throw new Error(`missing card ${title}`);
  return found;
}
function number(cardTitle: string, label: string): HTMLInputElement {
  const node = [...card(cardTitle).querySelectorAll("label")].find(
    (element) => element.textContent === label,
  );
  const input = node ? document.getElementById(node.getAttribute("for") ?? "") : null;
  if (!(input instanceof HTMLInputElement)) throw new Error(`missing ${label}`);
  return input;
}
async function input(cardTitle: string, label: string, value: string) {
  await flush(() => {
    const node = number(cardTitle, label);
    node.value = value;
    node.dispatchEvent(new Event("input", { bubbles: true }));
  });
}
function save(cardTitle: string) {
  return [...card(cardTitle).querySelectorAll("button")].find((button) =>
    button.textContent?.trim().startsWith("保存"),
  ) as HTMLButtonElement;
}
afterEach(() => {
  if (host) render(null, host);
  host?.remove();
  host = null;
  write.mockClear();
});

describe("设置页表单", () => {
  it("修改成员额度时刷新不覆盖草稿，保存完整数字与功能规则", async () => {
    reset();
    await mount();
    await input("成员额度", "V4.5 免费图/天", "250");
    store.rules = { ...store.rules, defaults: { ...store.rules.defaults, daily_images: 300 } };
    await mount();
    expect(number("成员额度", "V4.5 免费图/天").value).toBe("250");
    await flush(async () => {
      save("成员额度").click();
      await Promise.resolve();
    });
    expect(write).toHaveBeenCalledWith(
      "PUT",
      "/admin/api/member-rules",
      expect.objectContaining({ daily_images: 250, v5: false }),
      expect.any(String),
    );
    expect(store.rules.defaults.daily_images).toBe(250);
  });
  it("空值禁止保存，0 是明确停用", async () => {
    reset();
    await mount();
    await input("成员额度", "V4.5 免费图/天", "");
    expect(save("成员额度").disabled).toBe(true);
    await input("成员额度", "V4.5 免费图/天", "0");
    expect(save("成员额度").disabled).toBe(false);
    await flush(async () => {
      save("成员额度").click();
      await Promise.resolve();
    });
    expect(store.rules.defaults.daily_images).toBe(0);
  });
  it("全站预算只提交预算和运行开关", async () => {
    reset();
    await mount();
    await input("全站预算", "V5 全站每天", "0");
    await flush(async () => {
      save("全站预算").click();
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
  });
  it("领取条件按 0 = 不发放给提示", async () => {
    reset();
    await mount();
    expect(card("领取条件").textContent).toContain("0 = 不发放");
    expect(card("领取条件").textContent).toContain("9999 = 永久");
  });
});
