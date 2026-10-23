import { describe, expect, it } from "vitest";
import { type PoolToken, tokenFormatProblem, tokenState } from "./core";

const tok = (over: Partial<PoolToken>): PoolToken => ({
  token_id: "t",
  position: 1,
  token: "pst-ab…cd",
  usable: true,
  admin_enabled: true,
  disabled: false,
  fails: 0,
  blocked_for: 0,
  last_ok: 0,
  allow_anlas: false,
  image_concurrency: 1,
  image_active: 0,
  v5_daily_limit: 0,
  v5_used: 0,
  images_today: 0,
  ...over,
});

describe("令牌状态", () => {
  it("停用 > 失效 > 限流 > 正常", () => {
    expect(tokenState(tok({ admin_enabled: false, disabled: true })).text).toBe("已停用");
    expect(tokenState(tok({ disabled: true })).text).toBe("已失效（401）");
    expect(tokenState(tok({ blocked_for: 42 })).text).toBe("限流中 42 秒");
    expect(tokenState(tok({})).tone).toBe("ok");
  });
  it("今天出满了：不是「正常」，标「今日已满」", () => {
    expect(tokenState(tok({ full: true }))).toEqual({ text: "今日已满", tone: "warn" });
    expect(tokenState(tok({ full: true, blocked_for: 5 })).text).toBe("限流中 5 秒");
  });
  it("Token 格式", () => {
    expect(tokenFormatProblem("")).not.toBeNull();
    expect(tokenFormatProblem("abc")).toMatch(/pst-/);
    expect(tokenFormatProblem(" pst-abc ")).toBeNull();
  });
});
