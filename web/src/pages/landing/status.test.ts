import { describe, expect, it } from "vitest";
import { deriveStatus, generatedToday } from "./status";
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
  quota: { images_today: 10, v5_today: 2 },
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

  it("有人排队：只显示当前排队张数（站长 10-27），不显示秒数", () => {
    const v = deriveStatus(idle({ queue: { waiting: 3, running: 1 } }), { nowMs: 0 });
    expect(v?.tone).toBe("busy");
    expect(v?.headline).toBe("排队 3 张");
    expect(v?.announce).toBe("排队 3 张");
    expect(v?.sentence).not.toMatch(/秒|分钟|后开始/);
  });

  it("只有人在出图、没人排队：正在出图（还有空位，绿灯）", () => {
    const v = deriveStatus(idle({ queue: { waiting: 0, running: 2 } }));
    expect(v?.tone).toBe("ok");
    expect(v?.sentence).toBe("正在出图");
  });

  it("排队已满", () => {
    const v = deriveStatus(idle({ queue: { waiting: 5, running: 1, full: true } }));
    expect(v?.tone).toBe("full");
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

  it("我的图在排队：您排在 X/Y（X=位次，Y=当前排队张数；站长 10-27），不做时间估算", () => {
    const v = deriveStatus(
      idle({
        me: { mine: [{ state: "waiting", position: 4 }], waiting: 4, running: 1 },
      }),
    );
    expect(v?.sentence).toBe("您排在 4/4 位");
    expect(v?.announce).toBe("您排在 4/4 位");
    expect(v?.sentence).not.toMatch(/秒|分钟|后开始/);
  });

  it("即使全站被拦下，已经排上队的图仍显示自己的位置（与旧版一致）", () => {
    const v = deriveStatus(
      blocked("daily_cap", API.daily, {
        me: { mine: [{ state: "waiting", position: 1 }], waiting: 1, running: 0 },
      }),
    );
    expect(v?.headline).toBe("您排在 1/1 位");
  });
});

describe("generatedToday：图例只报今天各生成了多少张", () => {
  it("V4.5 = 总数 − V5，不出现负数；cap 是弧线比例尺", () => {
    expect(generatedToday(idle({ quota: { images_today: 10, v5_today: 3, images_cap: 4800 } }))).toEqual({
      total: 10,
      v45: 7,
      v5: 3,
      cap: 4800,
    });
  });
  it("V5 大于总数时按总数封顶（脏数据不产生负的 V4.5）", () => {
    expect(generatedToday(idle({ quota: { images_today: 2, v5_today: 5 } }))).toEqual({
      total: 2,
      v45: 0,
      v5: 2,
      cap: 0,
    });
  });
  it("没有数据全是 0", () => {
    expect(generatedToday(null)).toEqual({ total: 0, v45: 0, v5: 0, cap: 0 });
    expect(generatedToday(idle({ quota: {} }))).toEqual({ total: 0, v45: 0, v5: 0, cap: 0 });
  });
});
