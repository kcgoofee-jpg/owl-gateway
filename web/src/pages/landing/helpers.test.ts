import { describe, expect, it } from "vitest";
import { countdown, quotaRows, resetHour } from "./helpers";
import { mergeMeta } from "./store";
import type { MeMember } from "./types";

const member = (over: Partial<MeMember> = {}): MeMember => ({
  logged_in: true,
  has_key: true,
  today: {
    images: 30,
    v5: 4,
    legacy: 20,
    daily_images: 150,
    daily_v5: 6,
    anlas: 12,
    daily_anlas: 50,
    reset_hour: 7,
  },
  ...over,
});

describe("countdown", () => {
  const now = 1_000_000;
  it("hh:mm:ss，小时可以超过 24", () => {
    expect(countdown(now + 3 * 86400, now)).toBe("72:00:00");
    expect(countdown(now + 53 * 3600 + 5 * 60 + 9, now)).toBe("53:05:09");
  });
  it("不足一小时补零", () => {
    expect(countdown(now + 59, now)).toBe("00:00:59");
  });
  it("过期了是 00:00:00", () => {
    expect(countdown(now - 100, now)).toBe("00:00:00");
  });
});

describe("quotaRows", () => {
  it("V4.5 用 legacy 免费图张数；V5 / Anlas 各自有上限时显示", () => {
    const rows = quotaRows(member());
    expect(rows.map((r) => [r.id, r.used, r.cap])).toEqual([
      ["legacy", 20, 150],
      ["v5", 4, 6],
      ["anlas", 12, 50],
    ]);
  });
  it("没有 legacy 字段时退回 images − v5", () => {
    const rows = quotaRows(member({ today: { images: 30, v5: 4, daily_images: 150, daily_v5: 6 } }));
    expect(rows[0]?.used).toBe(26);
  });
  it("个人 V5 上限 0（10-27 = 停用）且模型范围是 all：仍显示，圆环暂停", () => {
    const v5 = quotaRows(
      member({ today: { images: 1, v5: 2, legacy: 1, daily_images: 150, daily_v5: 0 } }),
    ).find((r) => r.id === "v5");
    expect(v5?.cap).toBe(0);
    expect(v5?.paused).toBe(true);
    expect(v5?.hint).toContain("已停用");
  });
  it("没配 V5 上限且模型范围不是 all：不显示 V5；没配 Anlas 上限：不显示 Anlas", () => {
    const rows = quotaRows(
      member({
        today: { images: 1, v5: 0, legacy: 1, daily_images: 150, daily_v5: 0, daily_anlas: 0 },
      }),
    );
    expect(rows.map((r) => r.id)).toEqual(["legacy"]);
  });
  it("个人 V4.5 上限 0（10-27 = 停用）：圆环暂停", () => {
    const rows = quotaRows(member({ today: { images: 1, legacy: 1, daily_images: 0 } }));
    expect(rows[0]?.cap).toBe(0);
    expect(rows[0]?.paused).toBe(true);
  });
  it("V5 能用的是个人剩余和全站剩余的较小值；全站不设限只看个人", () => {
    const v5 = (today: Record<string, unknown>) =>
      quotaRows(
        member({ today: { images: 4, v5: 4, legacy: 0, daily_images: 150, daily_v5: 20, ...today } }),
      ).find((r) => r.id === "v5");
    expect(v5({ site_v5_left: 3, site_v5_cap: 300 })?.left).toBe(3);
    expect(v5({ site_v5_left: 100, site_v5_cap: 300 })?.left).toBe(16);
    expect(v5({ site_v5_left: null, site_v5_cap: null })?.left).toBeUndefined();
    expect(v5({ site_v5_left: null })?.paused).toBeFalsy();
  });
  it("全站 V5 用完：圆环暂停", () => {
    const rows = (today: Record<string, unknown>) =>
      quotaRows(
        member({
          today: { images: 0, v5: 2, legacy: 0, daily_images: 150, daily_v5: 20, ...today },
        }),
      ).find((r) => r.id === "v5");
    expect(rows({ site_v5_left: 0, site_v5_cap: 300 })?.paused).toBe(true);
    const open = rows({ site_v5_left: 3, site_v5_cap: 300 });
    expect([open?.cap, open?.used, open?.left, open?.paused]).toEqual([20, 2, 3, undefined]);
  });
});

describe("quotaRows: upstream V5 exhausted", () => {
  const v5 = (today: Record<string, unknown>) =>
    quotaRows(
      member({
        today: { images: 0, v5: 0, legacy: 0, daily_images: 150, daily_v5: 20, ...today },
      }),
    ).find((r) => r.id === "v5");
  it("false pauses the ring with a hint that names no reset time", () => {
    const row = v5({ upstream_v5_free: false });
    expect([row?.paused, row?.left]).toEqual([true, 0]);
    expect(row?.hint).toBe("全站 V5 免费额度暂时用完，正在逐步恢复，可以先用 V4.5");
    expect(v5({ upstream_v5_free: false, site_v5_left: 50, site_v5_cap: 300 })?.paused).toBe(true);
  });
  it("plan-paused V5 (battery low) pauses the ring with its own hint", () => {
    const row = v5({ v5_paused: true, upstream_v5_free: false });
    expect([row?.paused, row?.left]).toEqual([true, 0]);
    expect(row?.hint).toBe("V5 暂停，请使用 V4.5");
  });
  it("a missing field counts as true", () => {
    expect(v5({})?.paused).toBeUndefined();
    expect(v5({ upstream_v5_free: true })?.paused).toBeUndefined();
  });
});

describe("resetHour", () => {
  it("用接口给的整点，缺省按站长规定的 7 点", () => {
    expect(resetHour(member({ today: { reset_hour: 5 } }))).toBe(5);
    expect(resetHour(member({ today: {} }))).toBe(7);
    expect(resetHour(null)).toBe(7);
  });
});

describe("mergeMeta", () => {
  it("live 里有的字段优先，没有的用 /public/status 补", () => {
    const m = mergeMeta(
      { discord_invite: "https://discord.gg/a", registration: { open: false } },
      { registration: { open: true } },
    );
    expect(m).toEqual({
      discord_invite: "https://discord.gg/a",
      registration: { open: true },
    });
  });
  it("两边都没有就不出现该字段（不编默认值）", () => {
    expect(mergeMeta(null, null)).toEqual({});
  });
});
