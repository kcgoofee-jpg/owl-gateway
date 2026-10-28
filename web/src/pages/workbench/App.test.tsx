/** 生图工作台：入口闸门、策略门控、出图主链路（组合 + Bearer + 刷额度）、批量成功 / 失败暂停 / 限速重试。 */
import { render } from "preact";
import { act } from "preact/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { MemberPolicy } from "../../lib/member-policy";
import type { MeMember } from "../landing/types";
import { WorkbenchApp } from "./App";
import { me, reloadMe } from "./store";

// reloadMe 换成桩：出图成功后的额度刷新可断言；me 保持真 signal（测试里直接赋值）
vi.mock("./store", async (importOriginal) => {
  const orig = await importOriginal<typeof import("./store")>();
  return { ...orig, reloadMe: vi.fn(async () => {}) };
});

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
  tags: true,
  upscale: false,
  augment: false,
  img2img: false,
  paid_parameters: false,
};

const ME: MeMember = {
  logged_in: true,
  has_key: true,
  key: "k-test",
  policy: { ...POLICY },
  discord: { id: "1", username: "owl", display_name: "猫头鹰" },
  today: { images: 3, daily_images: 100, v5: 0, daily_v5: 0 },
};

const fetchMock = vi.fn();
const reloadMeMock = vi.mocked(reloadMe);

let host: HTMLElement | null = null;

async function mount(): Promise<HTMLElement> {
  const h = document.createElement("div");
  document.body.append(h);
  host = h;
  await act(async () => {
    render(<WorkbenchApp />, h);
    await Promise.resolve();
    await Promise.resolve();
  });
  return h;
}

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

const click = async (el: HTMLElement) => {
  await act(async () => {
    el.click();
    await Promise.resolve();
  });
};

const type = async (el: HTMLInputElement | HTMLTextAreaElement, text: string) => {
  await act(async () => {
    el.value = text;
    el.dispatchEvent(new Event("input", { bubbles: true }));
    await Promise.resolve();
  });
};

/** 找元素，找不到就抛错（测试里不给 ! 断言留位置）。 */
function need<T extends Element>(h: HTMLElement, selector: string): T {
  const el = h.querySelector<T>(selector);
  if (!el) throw new Error(`missing: ${selector}`);
  return el;
}

function buttonByText(h: HTMLElement, text: string): HTMLButtonElement {
  const el = [...h.querySelectorAll<HTMLButtonElement>("button")].find((b) => b.textContent === text);
  if (!el) throw new Error(`missing button: ${text}`);
  return el;
}

const okImage = () =>
  new Response(PNG_1X1 as BodyInit, { status: 200, headers: { "content-type": "binary/octet-stream" } });

const errJson = (status: number, error: Record<string, unknown>) =>
  new Response(JSON.stringify({ statusCode: status, error }), {
    status,
    headers: { "content-type": "application/json" },
  });

beforeEach(() => {
  fetchMock.mockReset();
  reloadMeMock.mockReset();
  reloadMeMock.mockResolvedValue(undefined);
  me.value = ME;
  vi.stubGlobal("fetch", fetchMock);
});

afterEach(() => {
  vi.unstubAllGlobals();
  host?.remove();
  host = null;
  me.value = undefined;
});

describe("WorkbenchApp", () => {
  it("游客只看到登录引导，不渲染工作台", async () => {
    me.value = { logged_in: false };
    const h = await mount();
    expect(h.textContent).toContain("先登录");
    expect(h.querySelector("textarea")).toBeNull();
  });

  it("成员默认出图页：模型 / 预设 / 提示词在位；没开放的能力不出现", async () => {
    const h = await mount();
    expect(h.textContent).toContain("Beta");
    expect(h.textContent).toContain("刷新即清空");
    // 免费档（img2img/vibe 关）：没有模式切换和参考图
    expect(h.querySelector(".wb-modes")).toBeNull();
    expect(h.textContent).not.toContain("参考图（Vibe）");
    // V5 没开：模型清单里没有 V5
    expect(need<HTMLSelectElement>(h, ".wb-side select").textContent).not.toContain("V5 Full");
    // tags 开着：标签补全在
    expect(h.textContent).toContain("标签补全");
    // 额度行来自 me.today
    expect(h.textContent).toContain("今日 3/100");
  });

  it("img2img 开放：出现模式切换；没传原图生成会拦下", async () => {
    me.value = { ...ME, policy: { ...POLICY, img2img: true } };
    const h = await mount();
    const modes = [...h.querySelectorAll(".wb-mode")].map((b) => b.textContent);
    expect(modes).toEqual(["文生图", "图生图", "局部重绘"]);
    await click([...h.querySelectorAll<HTMLButtonElement>(".wb-mode")][1] as HTMLButtonElement);
    await type(need<HTMLTextAreaElement>(h, ".wb-side textarea"), "same girl");
    await click(buttonByText(h, "生成"));
    await settle();
    expect(h.textContent).toContain("先传一张原图");
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("出图主链路：组合提示词、Bearer 头、成图进舞台与画廊、刷新额度", async () => {
    fetchMock.mockResolvedValueOnce(okImage());
    const h = await mount();
    await type(need<HTMLTextAreaElement>(h, ".wb-side textarea"), "1girl, seaside");
    // 画师词是侧栏第一个文本输入框；默认质量预设「无」→ 组合结果只有画师 + 提示词
    await type(need<HTMLInputElement>(h, ".wb-side input.input"), "artist:owl");
    await click(buttonByText(h, "生成"));
    await settle();
    const [, init] = fetchMock.mock.calls[0] as [string, RequestInit];
    expect(new Headers(init.headers).get("authorization")).toBe("Bearer k-test");
    const body = JSON.parse(String(init.body)) as { input: string; parameters: { negative_prompt: string } };
    expect(body.input).toBe("artist:owl, 1girl, seaside");
    // 默认负面预设「轻量」：预设词在前
    expect(body.parameters.negative_prompt).toContain("lowres");
    expect(h.querySelector(".wb-shot img")).toBeTruthy();
    expect(h.querySelectorAll(".wb-thumb")).toHaveLength(1);
    expect(reloadMeMock).toHaveBeenCalledTimes(1);
  });

  it("批量：两组各一张跑完进画廊，请求体带各组的词", async () => {
    fetchMock.mockResolvedValueOnce(okImage()).mockResolvedValueOnce(okImage());
    const h = await mount();
    await click(buttonByText(h, "批量出图"));
    const batch = need<HTMLElement>(h, 'section[aria-label="批量出图"]');
    const prompts = [...batch.querySelectorAll<HTMLTextAreaElement>("textarea")];
    await type(prompts[0] as HTMLTextAreaElement, "g one");
    await type(prompts[1] as HTMLTextAreaElement, "g two");
    await click(buttonByText(h, "批量开始"));
    await settle();
    await act(async () => {
      await new Promise((r) => setTimeout(r, 20));
    });
    await settle();
    expect(fetchMock).toHaveBeenCalledTimes(2);
    const b1 = JSON.parse(String((fetchMock.mock.calls[0] as [string, RequestInit])[1].body)) as {
      input: string;
    };
    const b2 = JSON.parse(String((fetchMock.mock.calls[1] as [string, RequestInit])[1].body)) as {
      input: string;
    };
    expect(b1.input).toContain("g one");
    expect(b2.input).toContain("g two");
    expect(h.textContent).toContain("已完成 2");
    expect(h.querySelectorAll(".wb-thumb")).toHaveLength(2);
    expect(reloadMeMock).toHaveBeenCalledTimes(2);
  });

  it("批量失败：任务标失败、队列暂停，出现「继续」", async () => {
    fetchMock.mockResolvedValueOnce(errJson(500, { message: "上游暂时不可用", status: 500 }));
    const h = await mount();
    await click(buttonByText(h, "批量出图"));
    const batch = need<HTMLElement>(h, 'section[aria-label="批量出图"]');
    const prompts = [...batch.querySelectorAll<HTMLTextAreaElement>("textarea")];
    await type(prompts[0] as HTMLTextAreaElement, "boom");
    await type(prompts[1] as HTMLTextAreaElement, "not yet");
    await click(buttonByText(h, "批量开始"));
    await settle();
    await act(async () => {
      await new Promise((r) => setTimeout(r, 20));
    });
    await settle();
    expect(h.querySelector(".wb-job--failed")?.textContent).toContain("上游暂时不可用");
    expect(h.textContent).toContain("队列已暂停");
    expect([...h.querySelectorAll("button")].some((b) => b.textContent === "继续")).toBe(true);
  });

  it("限速：按 Retry-After 自动重试一次后成功", async () => {
    fetchMock
      .mockResolvedValueOnce(
        new Response(
          JSON.stringify({
            statusCode: 429,
            error: { retryable: true, execution_unknown: false, message: "太快了" },
          }),
          {
            status: 429,
            headers: { "content-type": "application/json", "retry-after": "1" },
          },
        ),
      )
      .mockResolvedValueOnce(okImage());
    const h = await mount();
    await click(buttonByText(h, "批量出图"));
    const batch = need<HTMLElement>(h, 'section[aria-label="批量出图"]');
    await type(batch.querySelector<HTMLTextAreaElement>("textarea") as HTMLTextAreaElement, "slow down");
    await click(buttonByText(h, "批量开始"));
    await settle();
    await act(async () => {
      await new Promise((r) => setTimeout(r, 1200));
    });
    await settle();
    expect(fetchMock).toHaveBeenCalledTimes(2);
    expect(h.querySelector(".wb-job--done")).toBeTruthy();
    expect(h.textContent).toContain("已完成 1");
  });

  it("出图功能没开放：入口闸门拦截", async () => {
    me.value = { ...ME, policy: { ...POLICY, image: false } };
    const h = await mount();
    expect(h.textContent).toContain("出图功能没有开放给你的 Key");
    expect(h.querySelector("textarea")).toBeNull();
  });
});
