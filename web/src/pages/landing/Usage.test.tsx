import { render } from "preact";
import { afterEach, describe, expect, it } from "vitest";
import { StatRows } from "./Usage";

const host = document.createElement("div");

afterEach(() => render(null, host));

/** 站长 10-27：图例只说「今天重置以来各生成了多少张」，没有剩余额度、没有暂停态。 */
describe("StatRows: generated-today legend", () => {
  it("shows both generated counts and the composition arc", () => {
    render(<StatRows generated={{ total: 123, v45: 103, v5: 20 }} />, host);
    expect(host.querySelector(".stat--v45.lp-meter")?.textContent).toContain("V4.5 已出103张");
    expect(host.querySelector(".stat--v5.lp-meter")?.textContent).toContain("V5 已出20张");
    expect(host.querySelectorAll("svg.lp-site-gauge")).toHaveLength(1);
    expect(host.textContent).not.toContain("剩余");
    expect(host.textContent).not.toMatch(/已用 \d+%/);
  });

  it("draws V4.5 first from the origin, V5 right after it（黄前蓝后）", () => {
    render(<StatRows generated={{ total: 100, v45: 80, v5: 20 }} />, host);
    const all = host.querySelector("path.stat--v45");
    const subset = host.querySelector("path.stat--v5");
    expect(all?.getAttribute("stroke-dasharray")).toBe("80 100");
    expect(all?.getAttribute("stroke-dashoffset")).toBeNull();
    expect(subset?.getAttribute("stroke-dasharray")).toBe("20 100");
    expect(subset?.getAttribute("stroke-dashoffset")).toBe("-80");
    expect(subset?.getAttribute("d")).toBe(all?.getAttribute("d"));
  });

  it("a lone V5 image draws only the blue stretch, no amber invented", () => {
    render(<StatRows generated={{ total: 1, v45: 0, v5: 1 }} />, host);
    expect(host.querySelector("path.stat--v5")?.getAttribute("stroke-dasharray")).toBe("100 100");
    expect(host.querySelector("path.stat--v45")).toBeNull();
    expect(host.querySelector("svg title")?.textContent).toContain("今日已出 1 张，其中 V4.5 0 张、V5 1 张");
  });

  it("nothing generated yet: plain track, zeroed legend", () => {
    render(<StatRows generated={{ total: 0, v45: 0, v5: 0 }} />, host);
    expect(host.querySelector(".lp-site-gauge__fill")).toBeNull();
    expect(host.querySelector(".stat--v45.lp-meter")?.textContent).toContain("V4.5 已出0张");
  });
});
