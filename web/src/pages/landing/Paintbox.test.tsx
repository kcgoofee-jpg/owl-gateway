/** Paintbox：Beta 徽标、空提示词拦截、生成主链路（含钳制与刷新 me）、网关错误、标签补全。 */
import { signal } from "@preact/signals";
import { render } from "preact";
import { act } from "preact/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { MemberPolicy } from "../../lib/member-policy";
import { DEFAULT_FORM } from "../../lib/nai-config";
import { StoreCtx } from "./ctx";
import { Paintbox } from "./Paintbox";
import type { Store } from "./store";
import type { Live, MeMember } from "./types";

const PNG_1X1 = Uint8Array.from(
  atob("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII="),
  (c) => c.charCodeAt(0),
);

const POLICY: MemberPolicy = {
  daily_images: 100,
  daily_v5: 0,
  daily_anlas: 0,
  monthly_anlas: 0,
  rpm: 10,
  image: true,
  v5: false,
  vibe: false,
  tags: false,
  upscale: false,
  augment: false,
  img2img: false,
  paid_parameters: false,
};
const ME: MeMember = { logged_in: true, has_key: true, key: "k-test", policy: { ...POLICY } };

const fetchMock = vi.fn();
const reloadMe = vi.fn(async () => {});
const live = signal<Live>({ cycle: 40 });
const store = { live, reloadMe, connection: signal(true) } as unknown as Store;

let host: HTMLElement | null = null;

async function mount(me: MeMember = ME): Promise<HTMLElement> {
  const h = document.createElement("div");
  document.body.append(h);
  host = h;
  await act(async () => {
    render(
      <StoreCtx.Provider value={store}>
        <Paintbox me={me} />
      </StoreCtx.Provider>,
      h,
    );
    await Promise.resolve();
    await Promise.resolve();
  });
  return h;
}

/** 点完按钮后把 fetch 的微任务链和收尾重渲染冲干净。 */
const settle = async () => {
  await act(async () => {
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();
  });
  await act(async () => {
    await new Promise((r) => setTimeout(r, 0));
  });
};

const setPrompt = async (h: HTMLElement, text: string) => {
  const ta = h.querySelector<HTMLTextAreaElement>("textarea");
  if (!ta) throw new Error("textarea 缺失");
  await act(async () => {
    ta.value = text;
    ta.dispatchEvent(new Event("input", { bubbles: true }));
    await Promise.resolve();
  });
};

const click = async (el: HTMLElement) => {
  await act(async () => {
    el.click();
    await Promise.resolve();
  });
};

beforeEach(() => {
  fetchMock.mockReset();
  reloadMe.mockClear();
  live.value = { cycle: 40 };
  vi.stubGlobal("fetch", fetchMock);
});

afterEach(() => {
  vi.unstubAllGlobals();
  host?.remove();
  host = null;
});

const okImage = () =>
  new Response(PNG_1X1 as BodyInit, { status: 200, headers: { "content-type": "binary/octet-stream" } });
const errJson = (status: number, message: string) =>
  new Response(JSON.stringify({ statusCode: status, error: { message, status } }), {
    status,
    headers: { "content-type": "application/json" },
  });

describe("Paintbox", () => {
  it("卡片带 Beta 徽标，参数面板和提示在位；没开放的能力不出现", async () => {
    const h = await mount();
    expect(h.textContent).toContain("画匣");
    const badge = h.querySelector(".badge");
    expect(badge?.textContent).toBe("Beta");
    expect(h.querySelector(".card__sub")?.textContent).toContain("不用配置客户端");
    const model = h.querySelector<HTMLSelectElement>(".pb-grid select");
    expect(model?.textContent).toContain("V4.5 Full");
    expect(model?.textContent).not.toContain("V5 Full");
    expect(h.textContent).not.toContain("按这段补全标签");
  });

  it("空提示词不发请求，行内报错", async () => {
    const h = await mount();
    const btn = [...h.querySelectorAll("button")].find((b) => b.textContent === "生成");
    if (!btn) throw new Error("生成按钮缺失");
    await click(btn);
    await settle();
    expect(fetchMock).not.toHaveBeenCalled();
    expect(h.querySelector(".notice--danger")?.textContent).toContain("先写点提示词");
  });

  it("生成成功：请求带成员 Key、图渲染出来、历史入列、me 刷新一次", async () => {
    fetchMock.mockResolvedValueOnce(okImage()).mockResolvedValueOnce(okImage());
    const h = await mount();
    await setPrompt(h, "1girl, sunset");
    const btn = [...h.querySelectorAll("button")].find((b) => b.textContent === "生成");
    if (!btn) throw new Error("生成按钮缺失");
    await click(btn);
    await settle();
    expect(fetchMock).toHaveBeenCalledTimes(1);
    const [url, init] = fetchMock.mock.calls[0] as [string, RequestInit];
    expect(url).toBe("/ai/generate-image");
    expect((init.headers as Record<string, string>).Authorization).toBe("Bearer k-test");
    const body = JSON.parse(String(init.body)) as { model: string; parameters: { width: number } };
    expect(body.model).toBe(DEFAULT_FORM.model);
    expect(body.parameters.width).toBe(832);
    const img = h.querySelector<HTMLImageElement>(".pb-shot__img img");
    expect(img?.src.startsWith("data:image/png;base64,")).toBe(true);
    expect(reloadMe).toHaveBeenCalledTimes(1);
    const btn2 = [...h.querySelectorAll("button")].find((b) => b.textContent === "生成");
    if (!btn2) throw new Error("生成按钮缺失");
    await click(btn2);
    await settle();
    expect(reloadMe).toHaveBeenCalledTimes(2);
    expect(h.querySelectorAll(".pb-thumb")).toHaveLength(2);
    expect(h.textContent).toContain("刷新页面就清空");
  });

  it("网关错误原样给成员看", async () => {
    fetchMock.mockResolvedValueOnce(errJson(403, "这把 Key 已被临时暂停，未扣费。10-29 12:00 自动恢复。"));
    const h = await mount();
    await setPrompt(h, "1girl");
    const btn = [...h.querySelectorAll("button")].find((b) => b.textContent === "生成");
    if (!btn) throw new Error("生成按钮缺失");
    await click(btn);
    await settle();
    expect(h.querySelector(".notice--danger")?.textContent).toContain("临时暂停");
    expect(h.querySelector(".pb-shot__img")).toBeNull();
    expect(reloadMe).not.toHaveBeenCalled();
  });

  it("取消等待：说明已开画仍会计入额度，按钮恢复可点", async () => {
    fetchMock.mockImplementationOnce(
      (_url: string, init?: RequestInit) =>
        new Promise((_resolve, reject) => {
          init?.signal?.addEventListener("abort", () => reject(new DOMException("已中止。", "AbortError")));
        }),
    );
    const h = await mount();
    await setPrompt(h, "1girl");
    const btn = [...h.querySelectorAll("button")].find((b) => b.textContent === "生成");
    if (!btn) throw new Error("生成按钮缺失");
    await click(btn);
    const cancel = [...h.querySelectorAll("button")].find((b) => b.textContent === "取消等待");
    if (!cancel) throw new Error("取消按钮缺失");
    await click(cancel);
    await settle();
    expect(h.querySelector(".notice--warn")?.textContent).toContain("仍会计入今天的额度");
    const again = [...h.querySelectorAll("button")].find(
      (b) => b.textContent === "生成",
    ) as HTMLButtonElement;
    expect(again.disabled).toBe(false);
    expect(h.querySelector(".pb-shot__img")).toBeNull();
    expect(reloadMe).not.toHaveBeenCalled();
  });

  it("免费 Key 的步数与尺寸在送出前钳制，并写明", async () => {
    fetchMock.mockResolvedValueOnce(okImage());
    const h = await mount();
    await setPrompt(h, "1girl");
    const steps = h.querySelectorAll<HTMLInputElement>('.pb-grid input[type="number"]')[0];
    if (!steps) throw new Error("步数输入框缺失");
    await act(async () => {
      steps.value = "40";
      steps.dispatchEvent(new Event("input", { bubbles: true }));
      await Promise.resolve();
    });
    const btn = [...h.querySelectorAll("button")].find((b) => b.textContent === "生成");
    if (!btn) throw new Error("生成按钮缺失");
    await click(btn);
    await settle();
    const [, init] = fetchMock.mock.calls[0] as [string, RequestInit];
    const body = JSON.parse(String(init.body)) as { parameters: { steps: number } };
    expect(body.parameters.steps).toBe(28);
    expect(h.textContent).toContain("28 步");
  });

  it("V5 与标签补全按 Key 的策略开放：补全结果点一下进提示词", async () => {
    fetchMock.mockResolvedValueOnce(
      new Response(JSON.stringify({ tags: [{ tag: "1girl", count: 1000 }] }), {
        status: 200,
        headers: { "content-type": "application/json" },
      }),
    );
    const h = await mount({ ...ME, policy: { ...POLICY, v5: true, tags: true } });
    expect(h.querySelector<HTMLSelectElement>(".pb-grid select")?.textContent).toContain("V5 Full");
    await setPrompt(h, "girl at");
    const btn = [...h.querySelectorAll("button")].find((b) => b.textContent?.includes("按这段补全标签"));
    if (!btn) throw new Error("补全按钮缺失");
    await click(btn);
    await settle();
    const [url, init] = fetchMock.mock.calls[0] as [string, RequestInit];
    expect(url.startsWith("/ai/generate-image/suggest-tags?prompt=girl%20at")).toBe(true);
    expect((init.headers as Record<string, string>).Authorization).toBe("Bearer k-test");
    const chip = h.querySelector<HTMLButtonElement>(".pb-chip");
    expect(chip?.textContent).toBe("1girl");
    await click(chip as HTMLButtonElement);
    const ta = h.querySelector<HTMLTextAreaElement>("textarea");
    expect(ta?.value).toBe("girl at, 1girl");
  });

  it("Key 未开放出图：给闸门文案，不给表单", async () => {
    const h = await mount({ ...ME, requires_web_login: true });
    expect(h.querySelector(".notice--warn")?.textContent).toContain("Discord 登录");
    expect(h.querySelector(".pb")).toBeNull();
  });
});
