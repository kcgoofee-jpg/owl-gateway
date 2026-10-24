import { describe, expect, it } from "vitest";
import { dailyMeters, dailyUsage, deriveStatus } from "./status";
import type { Live } from "./types";

/**
 * 下面的 API 原文逐字来自 crates/gate-core/src/guard.rs（出图限量、排队满的拒绝原因）与 crates/gate-server/src/auth.rs（图片冷却）：
 * 成员收到的 429 原因和 /public/live 的 guard.blocked.text 是同一句。首页的说法必须和它同一口径（站长规定）。
 */
const API = {
  daily: "本站今天的出图总量用完了（为保护 NovelAI 账号，每个账号每天 1000 张），没有扣费。明天 7 点恢复",
  paced:
    "这个时段全站的出图份额用完了（每天 1000 张平均分到 24 小时，晚来的人也有图出），没有扣费。约 5 分钟后有空位",
  breaker: "NovelAI 连续出错，本站暂停出图，没有扣费。约 30 秒后自动恢复，请稍后再试",
  breakerTripped: "NovelAI 连续出错，本站暂停出图 60 秒，没有扣费。到点自动恢复，请稍后再试",
  cooldown: "NovelAI 那边正在限流，全站暂停出图约 45 秒，没有扣费。请稍后再试",
  queueFull: "现在排队的人太多（全站最多同时排 5 张），这张没有发出，没有扣费。请稍后再试",
};

const idle = (over: Partial<Live> = {}): Live => ({
  quota: { images_today: 10, images_cap: 1000, v5_today: 2, v5_cap: 63 },
  queue: { waiting: 0, running: 0 },
  guard: { cooldown: 0, blocked: null },
  rules: { queue_cap: 5 },
  cycle: 2.5,
  me: null,
  ...over,
});
const blocked = (code: string, text: string, over: Partial<Live> = {}): Live =>
  idle({ guard: { blocked: { code, text } }, ...over });

const sentence = (live: Live | null) => deriveStatus(live)?.sentence;

describe("deriveStatus：正常与排队", () => {
  it("没有数据时返回 null（页面显示骨架屏）", () => {
    expect(deriveStatus(null)).toBeNull();
  });

  it("空闲：马上开始 · 通道空闲", () => {
    const v = deriveStatus(idle());
    expect(v?.tone).toBe("ok");
    expect(v?.sentence).toBe("马上开始 · 通道空闲");
  });

  it("有人排队：状态 + 前面还有几张在等（站长 10-24），不显示秒数", () => {
    const v = deriveStatus(idle({ queue: { waiting: 3, running: 1 } }), { nowMs: 0 });
    expect(v?.tone).toBe("busy");
    expect(v?.headline).toBe("排队中");
    expect(v?.detail).toBe("前面还有 3 张在等");
    expect(v?.announce).toBe("排队中 · 前面还有 3 张在等");
    expect(v?.sentence).not.toMatch(/秒|分钟|后开始/);
  });

  it("没有放行间隔时同样带队列数量", () => {
    const v = deriveStatus(idle({ queue: { waiting: 2, running: 0 }, cycle: 0 }));
    expect(v?.sentence).toBe("排队中 · 前面还有 2 张在等");
  });

  it("排队已满", () => {
    const v = deriveStatus(idle({ queue: { waiting: 5, running: 1, full: true } }));
    expect(v?.tone).toBe("warn");
    expect(v?.headline).toBe("排队已满");
    expect(v?.detail).toContain("最多 5 张");
    expect(API.queueFull).toContain("最多同时排 5 张");
  });

  it("网关给了 queue.full 就以它为准，不自己算", () => {
    expect(deriveStatus(idle({ queue: { waiting: 1, running: 0, full: true } }))?.headline).toBe("排队已满");
    expect(deriveStatus(idle({ queue: { waiting: 9, running: 2, full: false } }))?.headline).not.toBe(
      "排队已满",
    );
  });
});

describe("deriveStatus：与 API 的 429 原因同一句话", () => {
  it("上游冷却：秒数同 auth.rs 的冷却原文", () => {
    const v = deriveStatus(idle({ guard: { cooldown: 45 } }));
    expect(v?.tone).toBe("bad");
    expect(v?.sentence).toBe("NovelAI 限流中 · 暂停约 45 秒");
    expect(API.cooldown).toContain("暂停出图约 45 秒");
  });

  it("每日上限：明天 7 点恢复", () => {
    expect(sentence(blocked("daily_cap", API.daily))).toBe("今日出图已满 · 明天 7 点恢复");
  });

  it("每日上限：零点到 7 点之间，网关原文说「今天 7 点」，首页照说", () => {
    expect(sentence(blocked("daily_cap", API.daily.replace("明天 7 点", "今天 7 点")))).toBe(
      "今日出图已满 · 今天 7 点恢复",
    );
  });

  it("每日上限的恢复整点取自接口原文，不写死", () => {
    expect(sentence(blocked("daily_cap", API.daily.replace("明天 7 点", "明天 5 点")))).toBe(
      "今日出图已满 · 明天 5 点恢复",
    );
  });

  it("按时段放行：约 5 分钟后有空位", () => {
    const v = deriveStatus(blocked("paced", API.paced));
    expect(v?.tone).toBe("paced");
    expect(v?.sentence).toBe("本时段份额已用完 · 约 5 分钟后有空位");
  });

  it("熔断：标题只说原因，灰字只留时间", () => {
    const v = deriveStatus(blocked("breaker", API.breaker));
    expect(v?.tone).toBe("bad");
    expect(v?.sentence).toBe("上游连续出错 · 约 30 秒后恢复");
    expect(sentence(blocked("breaker", API.breakerTripped))).toBe("上游连续出错 · 暂停 60 秒");
  });

  it("站长手动熔断写的原因：放在灰字里", () => {
    expect(sentence(blocked("breaker", "站长正在调整"))).toBe("暂停出图 · 站长正在调整");
  });

  it("不认识的原因码：原文照搬，不改写", () => {
    expect(sentence(blocked("something_new", "新规则：请稍后再试"))).toBe("新规则：请稍后再试");
  });

  it("优先级：冷却 > 全站拦截 > 排队满 > 排队", () => {
    const base = {
      queue: { waiting: 9, running: 0, full: true },
      guard: {
        cooldown: 10,
        blocked: { code: "paced", text: API.paced },
      },
    };
    expect(deriveStatus(idle(base))?.headline).toBe("NovelAI 限流中");
    expect(deriveStatus(idle({ ...base, guard: { ...base.guard, cooldown: 0 } }))?.headline).toBe(
      "本时段份额已用完",
    );
    expect(
      deriveStatus(idle({ ...base, guard: { ...base.guard, cooldown: 0, blocked: null } }))?.headline,
    ).toBe("排队已满");
  });
});

describe("deriveStatus：我的图优先", () => {
  it("我的图正在生成", () => {
    const v = deriveStatus(idle({ me: { mine: [{ state: "running" }], waiting: 0, running: 1 } }));
    expect(v?.tone).toBe("mine");
    expect(v?.sentence).toBe("你的图正在生成");
  });

  it("我的图只显示当前排位，不显示估算 ETA", () => {
    const v = deriveStatus(
      idle({
        me: { mine: [{ state: "waiting", position: 4, eta: 30 }], waiting: 4, running: 1 },
      }),
    );
    expect(v?.sentence).toBe("你的图排第 4 位");
    expect(v?.announce).toBe("你的图排第 4 位");
  });

  it("即使全站被拦下，已经排上队的图仍显示自己的位置（与旧版一致）", () => {
    const v = deriveStatus(
      blocked("daily_cap", API.daily, {
        me: { mine: [{ state: "waiting", position: 1, eta: 15 }], waiting: 1, running: 0 },
      }),
    );
    expect(v?.headline).toBe("你的图排第 1 位");
  });
});

describe("dailyUsage", () => {
  it("按实际张数；上限 0 = 不限", () => {
    const u = dailyUsage(idle({ quota: { images_today: 10, images_cap: 0, v5_today: 3, v5_cap: 0 } }));
    expect(u.total).toEqual({ used: 10, cap: 0 });
    expect(u.v5).toEqual({ used: 3, cap: 0 });
  });
  it("没有数据全是 0", () => {
    expect(dailyUsage(idle({ quota: { images_today: 7, images_cap: 1000 } })).total).toEqual({
      used: 7,
      cap: 1000,
    });
    expect(dailyUsage(null)).toEqual({ total: { used: 0, cap: 0 }, v5: { used: 0, cap: 0 } });
  });
});

// 北京时间 10-19 14:00 与 10-20 01:00：只有 V5 满了时没有 429 原文，「今天 / 明天」按北京时间算
const AFTERNOON = Date.parse("2026-10-19T14:00:00+08:00");
const AFTER_MIDNIGHT = Date.parse("2026-10-20T01:00:00+08:00");

describe("dailyMeters: a full meter shows tomorrow", () => {
  it("full total and full V5 switch to tomorrow's allowance; the reset hour comes from the 429 text", () => {
    const live = idle({
      quota: { images_today: 2803, images_cap: 2800, v5_today: 224, v5_cap: 224 },
      guard: { blocked: { code: "daily_cap", text: "本站今天的出图总量用完了，没有扣费。明天 7 点恢复" } },
    });
    const m = dailyMeters(live);
    expect(m.total).toEqual({ label: "全站明天可用（明天 7 点起）", used: 0, cap: 2800 });
    expect(m.v5).toEqual({ label: "V5 明天可用（明天 7 点起）", used: 0, cap: 224 });
  });
  it("only the full meter switches; unlimited and partly used stay as today", () => {
    const m = dailyMeters(
      idle({ quota: { images_today: 900, images_cap: 2800, v5_today: 224, v5_cap: 224 } }),
      AFTERNOON,
    );
    expect(m.total).toEqual({ label: "全站今日已用", used: 900, cap: 2800 });
    expect(m.v5.label).toBe("V5 明天可用（明天 7 点起）");
    expect(dailyMeters(idle({ quota: { images_today: 5, images_cap: 0 } })).total.label).toBe("全站今日已用");
  });
  it("after midnight and before the 7:00 reset, a full meter says today", () => {
    const v5Full = idle({ quota: { images_today: 900, images_cap: 2800, v5_today: 224, v5_cap: 224 } });
    expect(dailyMeters(v5Full, AFTER_MIDNIGHT).v5.label).toBe("V5 今天可用（今天 7 点起）");
    const allFull = idle({
      quota: { images_today: 2800, images_cap: 2800, v5_today: 224, v5_cap: 224 },
      guard: { blocked: { code: "daily_cap", text: "本站今天的出图总量用完了，没有扣费。今天 7 点恢复" } },
    });
    expect(dailyMeters(allFull, AFTERNOON).total.label).toBe("全站今天可用（今天 7 点起）");
  });
});

describe("dailyMeters: upstream V5 exhausted", () => {
  it("false pauses the V5 meter; missing or true leaves it alone", () => {
    const q = { images_today: 5, images_cap: 2800, v5_today: 10, v5_cap: 224 };
    expect(dailyMeters(idle({ quota: { ...q, upstream_v5_free: false } })).v5).toEqual({
      label: "V5 暂时用完",
      used: 10,
      cap: 224,
      paused: true,
    });
    expect(dailyMeters(idle({ quota: q })).v5.paused).toBeUndefined();
    // 站长手动暂停：同样是暂停的仪表，换成「暂停中」的说明
    expect(dailyMeters(idle({ quota: { ...q, upstream_v5_free: false, v5_paused: true } })).v5).toEqual({
      label: "V5 暂停中",
      used: 10,
      cap: 224,
      paused: true,
      hint: "V5 暂停，请使用 V4.5",
      short: "暂停中",
    });
    expect(dailyMeters(idle({ quota: { ...q, upstream_v5_free: true } })).v5.paused).toBeUndefined();
  });
});
