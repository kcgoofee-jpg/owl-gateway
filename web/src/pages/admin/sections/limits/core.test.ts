import { describe, expect, it } from "vitest";
import { guardSpecs, runtimeSpecs } from "./core";

describe("额度与保护：全是手动设置", () => {
  it("运行排队字段的范围取自接口", () => {
    const specs = runtimeSpecs({
      queue_timeout: 90,
      image_429_cooldown_seconds: 60,
      fields: {
        queue_timeout: { min: 15, max: 300 },
        image_429_cooldown_seconds: { min: 60, max: 3600 },
      },
    });
    expect(specs.queue_timeout).toMatchObject({ kind: "int", label: "排队超时", min: 15, max: 300 });
    expect(Object.keys(specs)).toEqual(["queue_timeout", "image_429_cooldown_seconds"]);
  });
  it("账号保护字段的标题和范围都取自接口，前端只补说明", () => {
    const specs = guardSpecs({
      values: { account_daily_cap: 2400 },
      fields: { account_daily_cap: { default: 2400, min: 0, max: 20000, label: "单号每天上限" } },
    });
    expect(specs.account_daily_cap).toMatchObject({ kind: "int", label: "单号每天上限", min: 0, max: 20000 });
    const cap = specs.account_daily_cap;
    if (cap?.kind !== "int") throw new Error("expected int");
    expect(cap.hint).toContain("20000 = 不限");
    expect(cap.hint).not.toContain("9999");
  });
});
