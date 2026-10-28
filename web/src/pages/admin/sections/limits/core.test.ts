import { describe, expect, it } from "vitest";
import { agoText, dash, guardSpecs, RUNTIME_SPECS } from "./core";

describe("额度与保护：全是手动设置", () => {
  it("运行排队字段保留服务端边界", () => {
    expect(RUNTIME_SPECS.queue_timeout).toMatchObject({ kind: "int", min: 15, max: 300 });
    expect(Object.keys(RUNTIME_SPECS)).toEqual(["queue_timeout", "image_429_cooldown_seconds"]);
  });
  it("账号保护字段的范围取自接口，标题取短名", () => {
    const specs = guardSpecs({
      values: { account_daily_cap: 2400 },
      fields: { account_daily_cap: { default: 2400, min: 0, max: 20000, label: "长说明" } },
    });
    expect(specs.account_daily_cap).toMatchObject({ kind: "int", label: "单号每天上限", min: 0, max: 20000 });
  });
  it("多久以前 / 空值", () => {
    const now = 1_000_000_000;
    expect(agoText(null, now)).toBe("没有");
    expect(agoText(now / 1000 - 30, now)).toBe("30 秒前");
    expect(agoText(now / 1000 - 7200, now)).toBe("2 小时前");
    expect(dash(null, "%")).toBe("—");
    expect(dash(40, "%")).toBe("40%");
  });
});
