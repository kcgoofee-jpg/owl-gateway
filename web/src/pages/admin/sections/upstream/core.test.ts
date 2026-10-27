import { describe, expect, it } from "vitest";
import {
  type AnlasEstimate,
  ageText,
  anlasText,
  answer,
  daysLeftText,
  duration,
  projectionText,
  reading,
  tokenFormatProblem,
  tokenState,
} from "./core";

describe("tokenState", () => {
  const base = { admin_enabled: true, disabled: false, blocked_for: 0, full: false };
  it("按停用 → 失效 → 限流 → 今日已满 → 正常排序", () => {
    expect(tokenState({ ...base, admin_enabled: false })).toEqual({ text: "已停用", tone: undefined });
    expect(tokenState({ ...base, disabled: true }).tone).toBe("danger");
    expect(tokenState({ ...base, blocked_for: 30 }).text).toBe("限流中 30 秒");
    expect(tokenState({ ...base, full: true }).text).toBe("今日已满");
    expect(tokenState(base)).toEqual({ text: "正常", tone: "ok" });
  });
});

describe("tokenFormatProblem", () => {
  it("要求非空且 pst- 开头", () => {
    expect(tokenFormatProblem("  ")).toContain("请粘贴");
    expect(tokenFormatProblem("sk-abc")).toContain("pst-");
    expect(tokenFormatProblem("pst-abc")).toBeNull();
  });
});

describe("readings text", () => {
  it("窗口统计与 Anlas 文案", () => {
    expect(anlasText(null)).toBe("—");
    expect(anlasText(1234.567)).toBe("1,234.57");
  });
  it("核对推算的续费与核对时间", () => {
    const est = (over: Partial<AnlasEstimate>): AnlasEstimate => ({
      cycle_rolled: false,
      balance: null,
      fixed: null,
      purchased: null,
      expires_at: null,
      days_left: null,
      avg_daily: 0,
      projection: "unknown",
      projection_value: null,
      snapshot_age_days: 0,
      stale: false,
      approximate: false,
      unattributed_anlas: 0,
      ...over,
    });
    expect(daysLeftText(est({ days_left: 0.4 }))).toBe("不到 1 天");
    expect(daysLeftText(est({ days_left: 9.8 }))).toBe("9 天");
    expect(daysLeftText(est({}))).toBe("—");
    expect(ageText(0.2)).toBe("今天核对过");
    expect(ageText(3.4)).toBe("3 天前核对");
  });
});

it("读数保留零与否定值，缺失值留空", () => {
  expect(reading(0, "%")).toBe("0%");
  expect(reading(null)).toBe("—");
  expect(answer(false, "是", "否")).toBe("否");
  expect(answer(null, "是", "否")).toBe("—");
  expect(duration(0)).toBe("0 秒");
  expect(duration(null)).toBe("—");
  expect(duration(3660)).toBe("1 小时 1 分钟");
  expect(projectionText(null)).toBe("—");
});
