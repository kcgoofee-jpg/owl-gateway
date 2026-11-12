import { describe, expect, it } from "vitest";
import {
  type AnlasEstimate,
  ageText,
  anlasText,
  answer,
  duration,
  limitAgo,
  officialV5Images,
  queueText,
  quietText,
  reading,
  shareHostProblem,
  shareTokenProblem,
  tokenFormatProblem,
  tokenState,
  v5Highlight,
  yesterdayText,
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

describe("shareTokenProblem / shareHostProblem", () => {
  it("拼车 Key 要 NAI- 开头", () => {
    expect(shareTokenProblem("")).toContain("请粘贴");
    expect(shareTokenProblem("pst-abc")).toContain("NAI-");
    expect(shareTokenProblem("NAI-abcdefghijklmnop")).toBeNull();
  });
  it("地址要 https 主机名，测试允许本机 http", () => {
    expect(shareHostProblem("")).toContain("请填写");
    expect(shareHostProblem("https://share.example")).toBeNull();
    expect(shareHostProblem("https://share.example/path")).toContain("路径");
    expect(shareHostProblem("http://example.com")).toContain("https");
    expect(shareHostProblem("http://127.0.0.1:18081")).toBeNull();
  });
});

describe("readings text", () => {
  it("窗口统计与 Anlas 文案", () => {
    expect(anlasText(null)).toBe("—");
    expect(anlasText(1234.567)).toBe("1,234.57");
  });
  it("余额读数的时刻文案", () => {
    const bal = (over: Partial<AnlasEstimate>): AnlasEstimate => ({
      balance: null,
      fixed: null,
      v5_percent: null,
      at: 0,
      snapshot_age_days: 0,
      stale: false,
      ...over,
    });
    expect(bal({ balance: 970 }).balance).toBe(970);
    expect(ageText(0.2)).toBe("刚刚");
    expect(ageText(3.4)).toBe("3 天前");
  });
});

it("账号卡片上的运行文案", () => {
  expect(yesterdayText(3129, 35)).toBe("3,129 张，V5 35 张");
  expect(yesterdayText(undefined, undefined)).toBe("—");
  expect(queueText(0, 6)).toBe("没有在等，最多 6 张");
  expect(queueText(5, null)).toBe("5 张在等");
  expect(limitAgo(null, 1_000)).toBe("没有");
  expect(limitAgo(1_000 - 120, 1_000)).toBe("2 分钟前");
  expect(quietText(10.78)).toBe("10.78% / 天（约 186 张）");
  expect(quietText(null)).toBe("—");
});

it("官网估张按 17.3×百分比四舍五入，和截图 98%→1695、11%→190 对得上", () => {
  expect(officialV5Images(98)).toBe(1695);
  expect(officialV5Images(11)).toBe(190);
  expect(officialV5Images(100)).toBe(1730);
});

it("常规电量为空时仍显示 Usage Boost 额外电量", () => {
  expect(v5Highlight(null).label).toBe("—");
  expect(v5Highlight({ percent: 0, purchased_percent: 98, is_negative: true })).toEqual({
    label: "0% + 98% 额外",
    value: 98,
    status: "额外电量",
    warn: false,
    images: 1695,
  });
  expect(v5Highlight({ percent: 0, purchased_percent: 0, is_negative: true })).toMatchObject({
    label: "0%",
    status: "负额度",
    warn: true,
    images: 0,
  });
  expect(v5Highlight({ percent: 40, purchased_percent: 0, is_negative: false })).toEqual({
    label: "40%",
    value: 40,
    status: "",
    warn: false,
    images: 692,
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
});
