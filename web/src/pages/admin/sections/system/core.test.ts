import { describe, expect, it } from "vitest";
import {
  bytesText,
  type FieldSpec,
  mbText,
  type NumSpec,
  parseField,
  passwordProblem,
  sameRaw,
  toRaw,
} from "./core";

const int: NumSpec = { kind: "int", label: "名额", min: 0, max: 1000 };
const flt: NumSpec = { kind: "float", label: "预算", min: 0, max: 1_000_000 };

describe("parseField（规则同后端 _num：空值 / 负数 / 小数 / 越界都拒绝）", () => {
  it("空值不能当成 0", () => {
    expect(parseField(int, "")).toEqual({ ok: false, error: "名额不能为空" });
    expect(parseField(int, "   ")).toEqual({ ok: false, error: "名额不能为空" });
  });
  it("负数、非数字、小数（整数字段）、越界", () => {
    expect(parseField(int, "-1")).toEqual({ ok: false, error: "名额不能是负数" });
    expect(parseField(int, "abc")).toEqual({ ok: false, error: "名额必须是数字" });
    expect(parseField(int, "1.5")).toEqual({ ok: false, error: "名额必须是整数" });
    expect(parseField(int, "1001")).toEqual({ ok: false, error: "名额必须在 0～1000 之间" });
    expect(parseField(int, "1e3")).toEqual({ ok: true, value: 1000 });
  });
  it("0 是合法的（代表不限）", () => {
    expect(parseField(int, "0")).toEqual({ ok: true, value: 0 });
    expect(parseField(flt, "12.5")).toEqual({ ok: true, value: 12.5 });
  });
  it("下限大于 0 时，低于下限给范围提示", () => {
    const s: NumSpec = { kind: "int", label: "间隔", min: 15, max: 120 };
    expect(parseField(s, "10")).toEqual({ ok: false, error: "间隔必须在 15～120 之间" });
  });
  it("文字字段：长度与格式", () => {
    const t: FieldSpec = {
      kind: "text",
      label: "服务器 ID",
      max: 21,
      pattern: /^\d{15,21}$/,
      patternMsg: "格式不对",
    };
    expect(parseField(t, "")).toEqual({ ok: true, value: "" });
    expect(parseField(t, "123")).toEqual({ ok: false, error: "格式不对" });
    expect(parseField(t, "123456789012345678")).toEqual({ ok: true, value: "123456789012345678" });
    const required: FieldSpec = { kind: "text", label: "标题", min: 1, max: 5 };
    expect(parseField(required, " ")).toEqual({ ok: false, error: "标题不能为空" });
    expect(parseField(required, "123456")).toEqual({ ok: false, error: "标题最多 5 个字" });
  });
});

describe("toRaw / sameRaw", () => {
  it("布尔接受 0 / 1", () => {
    expect(toRaw({ kind: "bool", label: "x" }, 1)).toBe(true);
    expect(toRaw({ kind: "bool", label: "x" }, 0)).toBe(false);
  });
  it("数字比较数值，不比较文字", () => {
    expect(sameRaw(int, "010", "10")).toBe(true);
    expect(sameRaw(int, "11", "10")).toBe(false);
  });
  it("服务端值本身越界（例如读到 0 但下限是 15）时，文字一样就算没改", () => {
    const s: NumSpec = { kind: "int", label: "间隔", min: 15, max: 120 };
    expect(sameRaw(s, "0", "0")).toBe(true);
    expect(sameRaw(s, "", "0")).toBe(false);
  });
});

describe("显示", () => {
  it("MB / 字节", () => {
    expect(mbText(1536)).toBe("1.5 GB");
    expect(mbText(512)).toBe("512 MB");
    expect(bytesText(0)).toBe("0 KB");
    expect(bytesText(3 * 1024 ** 2)).toBe("3.0 MB");
  });
});

describe("passwordProblem（同后端 change_password）", () => {
  it("规则", () => {
    expect(passwordProblem("", "abcdefghijkl", "abcdefghijkl")).toBe("请输入当前密码");
    expect(passwordProblem("old", "short", "short")).toBe("新密码至少需要 12 个字符");
    expect(passwordProblem("abcdefghijkl", "abcdefghijkl", "abcdefghijkl")).toBe("新密码不能与当前密码相同");
    expect(passwordProblem("old", "abcdefghijkl", "abcdefghijkm")).toBe("两次输入的新密码不一致");
    expect(passwordProblem("old", "abcdefghijkl", "abcdefghijkl")).toBeNull();
  });
});
