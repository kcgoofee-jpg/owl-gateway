import { render } from "preact";
import { act } from "preact/test-utils";
import { afterEach, expect, it, vi } from "vitest";
import { getJSON, sendJSON } from "../../lib/api";
import { Patron } from "./Patron";

vi.mock("../../lib/api", async (original) => ({
  ...(await original<object>()),
  getJSON: vi.fn(),
  sendJSON: vi.fn(),
}));

const FEATURES = {
  image: true,
  v5: false,
  vibe: true,
  tags: true,
  img2img: true,
  upscale: true,
  augment: false,
  paid_parameters: false,
};

const TOKEN = {
  fingerprint: "pk-syn…0001",
  is_share: true,
  enabled: true,
  admin_disabled: false,
  isolated: false,
  concurrency: 3,
  active: 1,
  fails: 0,
  blocked_for: 0,
  images_today: 4,
  v5_used: 0,
  v5_daily_limit: 0,
  anlas_used_today: 0,
};

function me(overrides: Record<string, unknown> = {}) {
  return {
    site_name: "猫头鹰公益站",
    welcome: "感谢你的支持。",
    queue_jump: true,
    self_service: true,
    portal_mode: "full",
    exclusive_account: true,
    account_hint: null,
    key_fingerprint: "nai-te…0001",
    token: TOKEN,
    features: FEATURES,
    ...overrides,
  };
}

let root: HTMLDivElement;
afterEach(() => {
  if (root) {
    render(null, root);
    root.remove();
  }
  vi.clearAllMocks();
});

async function show(data: Record<string, unknown>) {
  vi.mocked(getJSON).mockResolvedValue(data);
  root = document.createElement("div");
  document.body.append(root);
  await act(() => render(<Patron />, root));
}

function tokenSection(): HTMLElement {
  const el = root.querySelector('[aria-label="专属 Key"]');
  if (!el) throw new Error("专属 Key 分区未渲染");
  return el as HTMLElement;
}

it("full 模式展示专属 Key 详情，可启停、可换 Key", async () => {
  await show(me());
  const section = tokenSection();
  expect(section.textContent).toContain("pk-syn…0001");
  expect(section.textContent).toContain("1/3");
  expect(section.textContent).toContain("4 张");
  expect(section.textContent).toContain("启用中");
  expect(section.textContent).not.toContain("pk-synthetic-donated-share01");

  vi.mocked(sendJSON).mockResolvedValue(me({ token: { ...TOKEN, enabled: false, admin_disabled: true } }));
  const toggle = section.querySelector('[role="switch"]') as HTMLButtonElement;
  await act(async () => {
    toggle.click();
  });
  expect(sendJSON).toHaveBeenCalledWith("PUT", "/patron/api/me/token/enabled", { enabled: false });
  expect(tokenSection().textContent).toContain("已停用");

  vi.mocked(sendJSON).mockResolvedValue(me({ token: { ...TOKEN, fingerprint: "pk-new…0002" } }));
  const input = tokenSection().querySelector('input[type="password"]') as HTMLInputElement;
  await act(async () => {
    input.value = "  pk-synthetic-rotated-share03  ";
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
  const submit = [...tokenSection().querySelectorAll("button")].find((b) => b.textContent === "验证并更换");
  await act(async () => {
    submit?.click();
  });
  expect(sendJSON).toHaveBeenCalledWith("PUT", "/patron/api/me/token/rotate", {
    token: "pk-synthetic-rotated-share03",
  });
  expect(tokenSection().textContent).toContain("pk-new…0002");
});

it("basic 模式只看详情，没有启停和换 Key 控件", async () => {
  await show(me({ portal_mode: "basic" }));
  const section = tokenSection();
  expect(section.textContent).toContain("pk-syn…0001");
  expect(section.textContent).toContain("联系站长开通完整模式");
  expect(section.querySelector('[role="switch"]')).toBeNull();
  expect(section.querySelector('input[type="password"]')).toBeNull();
});

it("自助总开关关闭时即使是 full 也不给控件", async () => {
  await show(me({ self_service: false }));
  const section = tokenSection();
  expect(section.querySelector('[role="switch"]')).toBeNull();
  expect(section.querySelector('input[type="password"]')).toBeNull();
});

it("隔离与冷却给出明确提示", async () => {
  await show(me({ token: { ...TOKEN, isolated: true, enabled: false, blocked_for: 42 } }));
  const section = tokenSection();
  expect(section.textContent).toContain("已隔离");
  expect(section.textContent).toContain("可能已失效");
  expect(section.textContent).toContain("42 秒");
});

it("绑定的 Key 丢失时提示联系站长", async () => {
  await show(me({ token: { missing: true } }));
  expect(tokenSection().textContent).toContain("已不在令牌池里");
});

it("未绑定专属号时不渲染 Key 分区", async () => {
  await show(me({ token: null, exclusive_account: false }));
  expect(root.querySelector('[aria-label="专属 Key"]')).toBeNull();
});
