/** RiskSignals：按成员懒加载线索、状态分段、分页、确认 / 忽略弹框、非待复核直接重开。 */
import { render } from "preact";
import { act } from "preact/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { RiskSignals, riskActions } from "./RiskSignals";
import type { AbuseResp } from "./types";

const api = vi.hoisted(() => ({ get: vi.fn(), send: vi.fn() }));

vi.mock("../../../../lib/api", () => ({
  ApiError: class ApiError extends Error {
    constructor(
      public status: number,
      message: string,
    ) {
      super(message);
    }
  },
  getJSON: (...args: unknown[]) => api.get(...(args as [string])),
  sendJSON: (...args: unknown[]) => api.send(...args),
}));

// 不引真实的 Admin 模块（整只后台路由太重）：本地实现一个跟着 tick 刷新的 useAdmin
vi.mock("../../Admin", async () => {
  const { signal } = await import("@preact/signals");
  const { useEffect, useState } = await import("preact/hooks");
  const tick = signal(0);
  const busy = signal(0);
  function useAdmin<T>(url: string | null) {
    const [state, setState] = useState<{ data: T | null; error: string | null; loading: boolean }>({
      data: null,
      error: null,
      loading: !!url,
    });
    useEffect(() => {
      if (!url) return;
      let alive = true;
      api.get(url).then(
        (data: T) => alive && setState({ data, error: null, loading: false }),
        (e: unknown) =>
          alive &&
          setState((s) => ({ ...s, error: e instanceof Error ? e.message : String(e), loading: false })),
      );
      return () => {
        alive = false;
      };
    }, [url, tick.value]);
    return state;
  }
  return { tick, busy, useAdmin, act: vi.fn() };
});

const NOW = 1_800_000_000;
const SIGNAL = {
  id: 1,
  kind: "device_exact",
  kind_label: "同设备",
  subject: "111,222",
  key_ids: "3,77",
  score: 88,
  detail: {
    summary: "2 个账号在同一台设备上网页登录过",
    evidence: [{ k: "设备", v: "同一台" }],
  },
  status: "open" as const,
  first_ts: NOW - 86400,
  last_ts: NOW - 3600,
  accounts: [
    { key_id: 3, name: "alice", discord_id: "111", discord_name: "Alice", deleted: false },
    { key_id: 77, name: null, discord_id: null, discord_name: null, deleted: true },
  ],
  members: [
    {
      discord_id: "111",
      username: "alice",
      display_name: "Alice",
      avatar_url: "",
      key_id: 3,
      key_name: "k3",
      banned: false,
      ban_reason: null,
      first_login: null,
      last_login: null,
      created_at: null,
    },
    {
      discord_id: "222",
      username: "bob",
      display_name: "Bob",
      avatar_url: "",
      key_id: null,
      key_name: null,
      banned: true,
      ban_reason: "倒卖",
      first_login: null,
      last_login: null,
      created_at: null,
    },
  ],
};
const resp = (over: Partial<AbuseResp> = {}): AbuseResp => ({
  signals: [SIGNAL],
  open: 1,
  counts: { open: 1, confirmed: 0, dismissed: 0, expired: 0, total: 1 },
  total: 1,
  page: 1,
  per: 10,
  ...over,
});

let host: HTMLElement | null = null;
async function mount(): Promise<HTMLElement> {
  const h = document.createElement("div");
  document.body.append(h);
  host = h;
  await act(async () => {
    render(<RiskSignals dcId="111" now={NOW} />, h);
    await Promise.resolve();
    await Promise.resolve();
  });
  return h;
}
/** 点完按钮后把异步取数和重渲染冲干净：先放行 fetch 的微任务链，再用宏任务收尾一次重渲染。 */
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
afterEach(() => {
  if (host) {
    render(null, host);
    host.remove();
    host = null;
  }
  vi.clearAllMocks();
});

const buttons = (h: HTMLElement) => [...h.querySelectorAll("button")];
const btn = (h: HTMLElement, text: string) => {
  const b = buttons(h).find((x) => x.textContent?.trim().startsWith(text));
  if (!b) throw new Error(`没有「${text}」按钮：${buttons(h).map((x) => x.textContent)}`);
  return b;
};
/** 确认框里的按钮（卡片上有同名按钮时用它区分）。 */
const dlgBtn = (text: string) => {
  const dlg = (host as HTMLElement).querySelector("dialog") as HTMLDialogElement;
  const b = [...dlg.querySelectorAll("button")].find((x) => x.textContent?.trim().startsWith(text));
  if (!b) throw new Error(`确认框里没有「${text}」`);
  return b;
};
const seg = (h: HTMLElement) => h.querySelector("[aria-label='风险线索状态']") as HTMLElement;
const urls = () => api.get.mock.calls.map((c) => c[0] as string);
const body = (i = 0) => api.send.mock.calls[i]?.[2];

beforeEach(() => {
  api.get.mockImplementation(async () => resp());
  api.send.mockImplementation(async () => ({ id: 1, status: "confirmed", tagged: 1 }));
});

describe("riskActions", () => {
  it("待复核可以确认和忽略，其余状态只能重开", () => {
    expect(riskActions("open")).toEqual(["confirm", "dismiss"]);
    expect(riskActions("confirmed")).toEqual(["reopen"]);
    expect(riskActions("dismissed")).toEqual(["reopen"]);
    expect(riskActions("expired")).toEqual(["reopen"]);
  });
});

describe("RiskSignals", () => {
  it("挂载即按成员懒加载待复核，卡片有摘要、证据、账号、Key 和操作", async () => {
    const h = await mount();
    expect(urls()).toEqual(["/admin/api/abuse?member=111&status=open&page=1&per=10"]);
    const card = h.querySelector(".mb-issue") as HTMLElement;
    expect(card.textContent).toContain("同设备");
    expect(card.textContent).toContain("风险分 88");
    expect(card.textContent).toContain("2 个账号在同一台设备上网页登录过");
    expect(card.textContent).toContain("设备：同一台");
    expect(card.textContent).toContain("Bob（已封禁）");
    expect(card.textContent).toContain("#3 Alice");
    expect(card.textContent).toContain("#77 ?（已删）");
    expect([...card.querySelectorAll("button")].map((b) => b.textContent?.trim())).toEqual([
      "确认并标记",
      "忽略",
    ]);
    // 分段上带各状态计数
    expect([...seg(h).querySelectorAll("button")].map((b) => b.textContent)).toEqual([
      "待复核 1",
      "已确认 0",
      "已忽略 0",
      "全部 1",
    ]);
  });

  it("切状态回第一页重新拉取；空数据显示对应空文案", async () => {
    const h = await mount();
    await act(async () => {
      btn(seg(h), "已确认").click();
    });
    await settle();
    expect(urls()[1]).toBe("/admin/api/abuse?member=111&status=confirmed&page=1&per=10");
    api.get.mockImplementation(async () => resp({ signals: [], total: 0 }));
    await act(async () => {
      btn(seg(h), "全部").click();
    });
    await settle();
    expect(urls()[2]).toBe("/admin/api/abuse?member=111&status=all&page=1&per=10");
    expect(h.textContent).toContain("这个成员没有风险线索。");
  });

  it("确认要过确认框，写明不封禁，再提交 action=confirm", async () => {
    const h = await mount();
    await act(async () => {
      btn(h, "确认并标记").click();
    });
    const dlg = h.querySelector("dialog") as HTMLDialogElement;
    expect(dlg.textContent).toContain("不会封禁");
    expect(dlg.textContent).toContain("#3");
    // 已删除的 Key 不进打标清单
    expect(dlg.textContent).not.toContain("#77");
    api.send.mockClear();
    await act(async () => {
      dlgBtn("确认线索").click();
    });
    await settle();
    expect(api.send).toHaveBeenCalledTimes(1);
    expect(api.send.mock.calls[0]?.slice(0, 3)).toEqual([
      "POST",
      "/admin/api/abuse/1",
      { action: "confirm" },
    ]);
  });

  it("忽略也要确认，关联封禁号的线索写明忽略 = 放行", async () => {
    api.get.mockImplementation(async () =>
      resp({ signals: [{ ...SIGNAL, kind: "banned_link", kind_label: "封禁号关联" }] }),
    );
    const h = await mount();
    await act(async () => {
      btn(h, "忽略").click();
    });
    const dlg = h.querySelector("dialog") as HTMLDialogElement;
    expect(dlg.textContent).toContain("忽略 = 放行该浏览器领取 Key");
    await act(async () => {
      dlgBtn("忽略").click();
    });
    await settle();
    expect(body()).toEqual({ action: "dismiss" });
  });

  it("非待复核只有「重新打开」，直接提交不开框", async () => {
    api.get.mockImplementation(async () => resp({ signals: [{ ...SIGNAL, status: "dismissed" as const }] }));
    const h = await mount();
    const card = h.querySelector(".mb-issue") as HTMLElement;
    expect([...card.querySelectorAll("button")].map((b) => b.textContent?.trim())).toEqual(["重新打开"]);
    await act(async () => {
      btn(h, "重新打开").click();
    });
    await settle();
    expect(h.querySelector("dialog")).toBeNull();
    expect(body()).toEqual({ action: "reopen" });
  });

  it("确认失败时错误显示在框里，框不关", async () => {
    const h = await mount();
    api.send.mockImplementation(async () => {
      throw new Error("写库失败");
    });
    await act(async () => {
      btn(h, "确认并标记").click();
    });
    await act(async () => {
      dlgBtn("确认线索").click();
    });
    await settle();
    expect(h.querySelector("[role=alert]")?.textContent).toContain("写库失败");
  });

  it("总数超过一页时出分页，翻页带着新页码重新拉取", async () => {
    api.get.mockImplementation(async () =>
      resp({ total: 25, counts: { open: 25, confirmed: 0, dismissed: 0, expired: 0, total: 25 } }),
    );
    const h = await mount();
    expect(h.textContent).toContain("共 25");
    await act(async () => {
      (h.querySelector("[aria-label='第 2 页']") as HTMLElement).click();
    });
    await settle();
    expect(urls()[1]).toBe("/admin/api/abuse?member=111&status=open&page=2&per=10");
  });
});
