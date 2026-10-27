import { render } from "preact";
import { afterEach, describe, expect, it } from "vitest";
import { StatRows } from "./Usage";

const host = document.createElement("div");

afterEach(() => render(null, host));

/** 站长 10-27：图例只说「今天重置以来各生成了多少张」；弧线 = 已出 ÷ 各账号今日上限合计。 */
describe("StatRows: generated-today legend with cap arc", () => {
  it("shows both generated counts and a progress arc against the cap", () => {
    render(<StatRows generated={{ total: 123, v45: 103, v5: 20, cap: 4800 }} />, host);
    expect(host.querySelector(".stat--v45.lp-meter")?.textContent).toContain("V4.5 已出103张");
    expect(host.querySelector(".stat--v5.lp-meter")?.textContent).toContain("V5 已出20张");
    expect(host.querySelectorAll("svg.lp-site-gauge")).toHaveLength(1);
    expect(host.textContent).not.toContain("剩余");
    // 123 / 4800 ≈ 2.6% 的弧
    const arc = host.querySelector("path.stat--v45");
    expect(arc?.getAttribute("stroke-dasharray")).toBe("2.1458333333333335 100");
  });

  it("draws V4.5 first from the origin, V5 right after it（黄前蓝后）", () => {
    render(<StatRows generated={{ total: 100, v45: 80, v5: 20, cap: 4800 }} />, host);
    const all = host.querySelector("path.stat--v45");
    const subset = host.querySelector("path.stat--v5");
    expect(Number(all?.getAttribute("stroke-dasharray")?.split(" ")[0])).toBeCloseTo(1.667, 2);
    const v5Dash = Number(subset?.getAttribute("stroke-dasharray")?.split(" ")[0]);
    expect(v5Dash).toBeCloseTo(0.4167, 2);
    expect(Number(subset?.getAttribute("stroke-dashoffset"))).toBeCloseTo(-1.667, 2);
    expect(subset?.getAttribute("d")).toBe(all?.getAttribute("d"));
  });

  it("never draws beyond the track when the cap is exceeded", () => {
    render(<StatRows generated={{ total: 100, v45: 90, v5: 20, cap: 100 }} />, host);
    expect(host.querySelector("path.stat--v45")?.getAttribute("stroke-dasharray")).toBe("90 100");
    expect(host.querySelector("path.stat--v5")?.getAttribute("stroke-dasharray")).toBe("10 100");
  });

  it("no cap (no open accounts): plain track, zeroed legend, no invented arc", () => {
    render(<StatRows generated={{ total: 5, v45: 5, v5: 0, cap: 0 }} />, host);
    expect(host.querySelector(".lp-site-gauge__fill")).toBeNull();
    expect(host.querySelector(".stat--v45.lp-meter")?.textContent).toContain("V4.5 已出5张");
    expect(host.querySelector("svg title")?.textContent).toContain("今日已出 5 张");
  });
});
