import { describe, expect, it } from "vitest";
import { scopeText } from "./format";

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
