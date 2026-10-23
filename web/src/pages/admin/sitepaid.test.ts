import { describe, expect, it } from "vitest";
import { defaultBudget, manualBody, parseBudget, type SitepaidHint } from "./sitepaid";

const hint: SitepaidHint = {
  token_id: "tok-1",
  expires_at: 1800000000,
  days_left: 2,
  fixed: 812.6,
  stale: false,
  manual: true,
  setting_key: "v5_paid_tokens",
  message: "m",
};

describe("sitepaid hint", () => {
  it("budget defaults to the floored remaining Anlas, empty when unknown", () => {
    expect(defaultBudget(hint)).toBe("812");
    const { fixed: _f, ...rest } = hint;
    expect(defaultBudget(rest)).toBe("");
  });
  it("budget must be a positive number", () => {
    expect(parseBudget(" 500 ")).toBe(500);
    for (const bad of ["", "0", "-3", "abc"]) expect(parseBudget(bad)).toBeNull();
  });
  it("manual body sets only this account until its expiry", () => {
    expect(manualBody(hint, 500, "why")).toEqual({
      key: "v5_paid_tokens",
      value: { "tok-1": { until: 1800000000, max_anlas: 500 } },
      why: "why",
    });
  });
});
