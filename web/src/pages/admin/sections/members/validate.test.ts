import { describe, expect, it } from "vitest";
import { isDiscordId, parseNum } from "./validate";

describe("parseNum", () => {
  it("空值、纯空白一律拒绝", () => {
    expect(parseNum("", "每日图片", { max: 10 })).toEqual({ ok: false, error: "每日图片不能为空" });
    expect(parseNum("   ", "每日图片", { max: 10 }).ok).toBe(false);
  });
  it("负数、小数（整数字段）、越界拒绝", () => {
    expect(parseNum("-1", "x", { max: 10 }).ok).toBe(false);
    expect(parseNum("1.5", "x", { max: 10 })).toEqual({ ok: false, error: "x必须是整数" });
    expect(parseNum("11", "x", { max: 10 }).ok).toBe(false);
    expect(parseNum("abc", "x", { max: 10 }).ok).toBe(false);
    expect(parseNum("Infinity", "x", { max: 10 }).ok).toBe(false);
  });
  it("0 是合法值（表示不限），非整数字段允许小数", () => {
    expect(parseNum("0", "x", { max: 10 })).toEqual({ ok: true, value: 0 });
    expect(parseNum("2.5", "Anlas", { max: 10, int: false })).toEqual({ ok: true, value: 2.5 });
  });
  it("rpm 的下限是 1", () => {
    expect(parseNum("0", "RPM", { min: 1, max: 600 }).ok).toBe(false);
    expect(parseNum("600", "RPM", { min: 1, max: 600 }).ok).toBe(true);
  });
});

describe("isDiscordId", () => {
  it("17～20 位纯数字", () => {
    expect(isDiscordId("900000000000000000")).toBe(true);
    expect(isDiscordId("1234567890123456")).toBe(false);
    expect(isDiscordId("12345678901234567890a")).toBe(false);
  });
});
