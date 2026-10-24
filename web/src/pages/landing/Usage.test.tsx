import { render } from "preact";
import { afterEach, describe, expect, it } from "vitest";
import type { MeterRow } from "./status";
import { StatRows } from "./Usage";

const host = document.createElement("div");
const row = (over: Partial<MeterRow> & { kind: "v45" | "v5" }): MeterRow => ({
  used: 0,
  cap: 0,
  ...over,
});

afterEach(() => render(null, host));

describe("status card meters: one caliber, images generated today", () => {
  it("both numbers are what was generated today; cap and reset live in the title", () => {
    render(
      <StatRows
        rows={[row({ kind: "v45", used: 8478, cap: 64000 }), row({ kind: "v5", used: 114, cap: 64000 })]}
        reset="明天 7 点"
      />,
      host,
    );
    const v45 = host.querySelector('.stat--v45[role="progressbar"]');
    expect(v45?.getAttribute("aria-label")).toBe("V4.5");
    expect(v45?.getAttribute("aria-valuenow")).toBe("8478");
    expect(v45?.getAttribute("aria-valuemax")).toBe("64000");
    expect(v45?.getAttribute("aria-valuetext")).toBe("今日已出 8,478 / 64,000 张，明天 7 点重置");
    expect(v45?.getAttribute("title")).toBe("今日已出 8,478 / 64,000 张，明天 7 点重置");
    expect(v45?.textContent).toContain("V4.58,478张");
    const v5 = host.querySelector(".stat--v5.lp-meter");
    expect(v5?.textContent).toContain("V5114张");
    expect(v5?.getAttribute("title")).toBe("全站今日 V5 共 114 张");
    expect(host.textContent).not.toContain("剩余");
    expect(host.textContent).not.toContain("明天可用");
  });

  it("draws one inline bar per row on the same site-cap scale, V4.5 first", () => {
    render(
      <StatRows
        rows={[row({ kind: "v45", used: 100, cap: 500 }), row({ kind: "v5", used: 20, cap: 500 })]}
        reset="明天 7 点"
      />,
      host,
    );
    const bars = host.querySelectorAll(".lp-meter__bar");
    expect(bars).toHaveLength(2);
    expect(bars[0]?.closest(".lp-meter")?.classList.contains("stat--v45")).toBe(true);
    expect(bars[1]?.closest(".lp-meter")?.classList.contains("stat--v5")).toBe(true);
    expect(bars[0]?.querySelector(".lp-meter__bar-fill")?.getAttribute("width")).toBe("9.6");
    expect(bars[1]?.querySelector(".lp-meter__bar-fill")?.getAttribute("width")).toBe("1.9");
  });

  it("keeps the shared site arc above the rows: amber total first, blue V5 over its head", () => {
    render(
      <StatRows
        rows={[row({ kind: "v45", used: 100, cap: 500 }), row({ kind: "v5", used: 20, cap: 500 })]}
        reset="明天 7 点"
      />,
      host,
    );
    const arc = host.querySelector("svg.lp-site-gauge");
    expect(arc?.getAttribute("viewBox")).toBe("0 0 1000 40");
    expect(arc?.getAttribute("aria-hidden")).toBe("true");
    expect(arc?.querySelector("title")?.textContent).toBe("全站今日已出 100 / 500 张（含 V5 20 张）");
    const fills = arc?.querySelectorAll(".lp-site-gauge__fill");
    expect(fills).toHaveLength(2);
    expect(fills?.[0]?.classList.contains("stat--v45")).toBe(true);
    expect(fills?.[0]?.getAttribute("stroke-dasharray")).toBe("20 100");
    expect(fills?.[1]?.classList.contains("stat--v5")).toBe(true);
    expect(fills?.[1]?.getAttribute("stroke-dasharray")).toBe("4 100");
  });

  it("does not invent a progressbar or bar for an unlimited allowance, title keeps the plain sentence", () => {
    render(
      <StatRows
        rows={[row({ kind: "v45", used: 123, cap: 0 }), row({ kind: "v5", used: 106, cap: 0 })]}
        reset="明天 7 点"
      />,
      host,
    );
    expect(host.querySelector("[role=progressbar]")).toBeNull();
    expect(host.querySelector(".lp-meter__bar")).toBeNull();
    // 上限 0：弧线只剩轨道，不编填充
    expect(host.querySelectorAll(".lp-site-gauge__fill")).toHaveLength(0);
    expect(host.querySelector(".lp-site-gauge title")?.textContent).toBe(
      "全站今日已出 123 张（含 V5 106 张）",
    );
    const v45 = host.querySelector(".stat--v45.lp-meter");
    expect(v45?.getAttribute("title")).toBe("今日已出 123 张");
    expect(v45?.textContent).toContain("V4.5123张");
    expect(host.querySelector(".stat--v5.lp-meter")?.getAttribute("title")).toBe("全站今日 V5 共 106 张");
    expect(host.textContent).not.toContain("不限量");
  });

  it("full site: caption names the reset, bar is full, number equals the cap", () => {
    render(
      <StatRows
        rows={[row({ kind: "v45", used: 2800, cap: 2800 }), row({ kind: "v5", used: 224, cap: 2800 })]}
        reset="明天 7 点"
      />,
      host,
    );
    const v45 = host.querySelector(".stat--v45.lp-meter");
    expect(v45?.querySelector(".lp-meter__caption")?.textContent).toBe("今日已满，明天 7 点重置");
    expect(v45?.classList.contains("stat--danger")).toBe(true);
    expect(v45?.querySelector(".lp-meter__bar-fill")?.getAttribute("width")).toBe("48");
    expect(v45?.textContent).toContain("V4.52,800张");
  });

  it("close to the cap turns warn in class as well as color", () => {
    render(
      <StatRows
        rows={[row({ kind: "v45", used: 480, cap: 500 }), row({ kind: "v5", used: 10, cap: 500 })]}
        reset="明天 7 点"
      />,
      host,
    );
    expect(host.querySelector(".stat--v45.lp-meter")?.classList.contains("stat--warn")).toBe(true);
    expect(host.querySelector(".stat--v5.lp-meter")?.classList.contains("stat--warn")).toBe(false);
  });

  it("paused V5: empty blue bar, generated count stays, caption and hint explain the pause", () => {
    render(
      <StatRows
        rows={[
          row({ kind: "v45", used: 100, cap: 500 }),
          row({ kind: "v5", used: 10, cap: 500, paused: true, hint: "V5 暂停，请使用 V4.5" }),
        ]}
        reset="明天 7 点"
      />,
      host,
    );
    const v5 = host.querySelector(".lp-meter--paused");
    expect(v5?.classList.contains("stat--v5")).toBe(true);
    expect(v5?.querySelector(".lp-meter__bar-fill")).toBeNull();
    expect(v5?.querySelector(".lp-meter__caption")?.textContent).toBe("V5 暂停中");
    expect(v5?.textContent).toContain("V510张");
    expect(v5?.getAttribute("title")).toBe("全站今日 V5 共 10 张；V5 暂停，请使用 V4.5");
    expect(v5?.getAttribute("aria-valuenow")).toBe("10");
  });
});
