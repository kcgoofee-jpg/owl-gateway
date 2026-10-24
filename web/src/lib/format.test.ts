import { describe, expect, it } from "vitest";
import { maskIp, scopeText, secondsText, usedOf, withUnit } from "./format";

describe("scopeText", () => {
  it("没写范围 = 只有 V4.5", () => {
    expect(scopeText()).toBe("V4.5");
    expect(scopeText("v45")).toBe("V4.5");
  });
  it("按顺序拼出 V5 / Anlas", () => {
    expect(scopeText("v45,v5,anlas")).toBe("V4.5、V5、Anlas");
    expect(scopeText("v45, anlas")).toBe("V4.5、Anlas");
  });
});

describe("后台数字/单位/耗时/IP", () => {
  it("数量 + 单位：千分位、空值", () => {
    expect(withUnit(4, "张")).toBe("4 张");
    expect(withUnit(1500, "张")).toBe("1,500 张");
    expect(withUnit(null, "张")).toBe("—");
  });
  it("已用 / 上限：上限 0 或空 = 不限", () => {
    expect(usedOf(4, 300)).toBe("4 / 300");
    expect(usedOf(4, 0)).toBe("4 / 不限");
    expect(usedOf(null, null)).toBe("0 / 不限");
  });
  it("毫秒 → 秒：≥10 秒取整", () => {
    expect(secondsText(4200)).toBe("4.2 秒");
    expect(secondsText(9000)).toBe("9.0 秒");
    expect(secondsText(12500)).toBe("13 秒");
    expect(secondsText(0)).toBe("—");
  });
  it("IP 默认打码：IPv4 留前两段，IPv6 留前两组", () => {
    expect(maskIp("103.151.22.8")).toBe("103.151.*.*");
    expect(maskIp("103.151.*.*")).toBe("103.151.*.*");
    expect(maskIp("2001:db8:1:2::3")).toBe("2001:db8:*");
    expect(maskIp("::1")).toBe("::*");
    expect(maskIp("unknown")).toBe("*");
  });
});
