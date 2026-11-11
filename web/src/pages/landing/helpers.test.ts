import { describe, expect, it } from "vitest";
import { countdown, quotaRows, resetHour } from "./helpers";
import { mergeMeta } from "./store";
import type { MeMember, QuotaReason, ServerQuotaRow } from "./types";

const member = (over: Partial<MeMember> = {}): MeMember => ({
  logged_in: true,
  has_key: true,
  today: { reset_hour: 7 },
  ...over,
});

const withRows = (rows: ServerQuotaRow[], reset_hour = 7) => member({ today: { reset_hour, rows } });

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
  it("照服务端的行填标签，不再自己算剩余", () => {
    const rows = quotaRows(
      withRows([
        { id: "legacy", used: 20, cap: 150, left: 130 },
        { id: "v5", used: 4, cap: 6, left: 2 },
        { id: "anlas", used: 12, cap: 50, left: 38 },
      ]),
    );
    expect(rows.map((r) => [r.id, r.label, r.used, r.cap, r.left])).toEqual([
      ["legacy", "V4.5", 20, 150, 130],
      ["v5", "V5", 4, 6, 2],
      ["anlas", "Anlas", 12, 50, 38],
    ]);
  });
  it("没有 rows 就不显示", () => {
    expect(quotaRows(member())).toEqual([]);
  });
  it("个人额度为 0：停用文案", () => {
    const rows = quotaRows(
      withRows([
        { id: "legacy", used: 1, cap: 0, left: 0, paused: true, reason: "disabled" },
        { id: "v5", used: 2, cap: 0, left: 0, paused: true, reason: "disabled" },
      ]),
    );
    expect(rows[0]?.hint).toBe("V4.5 已停用（个人额度为 0）");
    expect(rows[1]?.hint).toContain("已停用");
    expect(rows[1]?.paused).toBe(true);
  });
  it("全站池停用和用完不是同一句话", () => {
    const v5 = (reason: QuotaReason, cap = 20) =>
      quotaRows(withRows([{ id: "v5", used: 2, cap, left: 0, paused: true, reason }])).find(
        (r) => r.id === "v5",
      );
    expect(v5("disabled")?.hint).toBe("全站 V5 已停用");
    expect(v5("site_empty")?.hint).toBe("全站今天的 V5 额度用完了，7 点重置后恢复");
    expect(v5("member_off")?.hint).toBe("此成员未开放 V5");
    expect(v5("site_paused")?.hint).toBe("V5 暂停，请使用 V4.5");
    expect(v5("upstream_exhausted")?.hint).toBe("全站 V5 免费额度暂时用完，正在逐步恢复，可以先用 V4.5");
  });
  it("没有原因码就不加说明，剩余原样显示", () => {
    const row = quotaRows(withRows([{ id: "v5", used: 2, cap: 20, left: 3 }]))[0];
    expect(row?.hint).toBeUndefined();
    expect(row?.paused).toBeUndefined();
    expect(row?.left).toBe(3);
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

describe("anlas 原因码", () => {
  it("全站池用完时带重置时间", () => {
    const row = quotaRows(
      withRows([{ id: "anlas", used: 181, cap: 300, left: 0, paused: true, reason: "site_empty" }]),
    )[0];
    expect(row?.paused).toBe(true);
    expect(row?.left).toBe(0);
    expect(row?.hint).toContain("全站今天的 Anlas 额度用完了");
  });
  it("全站池停用时不说用完", () => {
    const row = quotaRows(
      withRows([{ id: "anlas", used: 0, cap: 300, left: 0, paused: true, reason: "disabled" }]),
    )[0];
    expect(row?.hint).toBe("全站 Anlas 已停用");
  });
});
