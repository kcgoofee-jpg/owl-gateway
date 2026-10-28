import { describe, expect, it } from "vitest";
import { exceptionCount, type MemberPolicy, overridesOf } from "./member-policy";

const defaults: MemberPolicy = {
  daily_images: 100,
  daily_v5: 5,
  daily_anlas: 15,
  monthly_anlas: 99999,
  rpm: 10,
  image: true,
  v5: false,
  vibe: true,
  tags: true,
  upscale: false,
  augment: false,
  img2img: false,
  paid_parameters: false,
};
describe("成员字段例外", () => {
  it("同值清空，零和 false 保留为明确例外", () => {
    const overrides = overridesOf(defaults, { ...defaults, daily_images: 0, image: false, v5: true });
    expect(overrides.daily_images).toBe(0);
    expect(overrides.image).toBe(false);
    expect(overrides.v5).toBe(true);
    expect(overrides.daily_anlas).toBeNull();
    expect(exceptionCount(overrides)).toBe(3);
  });
  it("恢复默认提交空值，不固定旧默认值", () => {
    expect(exceptionCount(overridesOf(defaults, defaults))).toBe(0);
    expect(Object.values(overridesOf(defaults, defaults)).every((value) => value === null)).toBe(true);
  });
});
