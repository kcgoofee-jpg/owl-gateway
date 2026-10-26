import { describe, expect, it } from "vitest";
import { agoText, dash, guardSpecs, QUOTA_SPECS, RUNTIME_SPECS } from "./core";

describe("额度与保护：全是手动设置", () => {
  it("每人 / 全站额度的范围与后端一致", () => {
    expect(QUOTA_SPECS.register_daily_images).toMatchObject({ kind: "int", min: 0, max: 9999 });
    expect(QUOTA_SPECS.global_monthly_anlas?.kind).toBe("float");
    // 自动档已删除：只有关 / 开
    const medium = QUOTA_SPECS.v5_medium_mode;
    expect(medium?.kind === "select" && medium.options.map((o) => o.value)).toEqual(["off", "on"]);
    // 出图间隔 10-23 删除：节奏只剩账号保护的「每张出完后歇 N～M 秒」
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
