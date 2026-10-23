import { render } from "preact";
import { afterEach, describe, expect, it } from "vitest";
import { StatRows } from "./Usage";

const host = document.createElement("div");
const v5 = { label: "V5 已用", used: 0, cap: 0 };

afterEach(() => render(null, host));

describe("compact shared allowance arc", () => {
  it("exposes remaining images and capacity to sighted users and screen readers", () => {
    render(<StatRows total={{ label: "全站今日已用", used: 123, cap: 500 }} v5={v5} />, host);
    const gauge = host.querySelector('.stat--v45[role="progressbar"]');
    expect(gauge?.getAttribute("aria-label")).toBe("V4.5");
    expect(gauge?.getAttribute("aria-valuenow")).toBe("377");
    expect(gauge?.getAttribute("aria-valuemax")).toBe("500");
    expect(gauge?.getAttribute("aria-valuetext")).toContain("377 张，每日 500 张");
    expect(gauge?.textContent).toContain("V4.5377张");
    expect(host.querySelectorAll("svg.lp-site-gauge")).toHaveLength(1);
    expect(host.textContent).not.toContain("全站剩余");
    expect(host.textContent).not.toMatch(/已用 \d+%/);
  });

  it("keeps the next reset explicit instead of promising an available full allowance", () => {
    render(<StatRows total={{ label: "全站明天可用（明天 7 点起）", used: 0, cap: 500 }} v5={v5} />, host);
    const gauge = host.querySelector(".stat--v45.lp-meter");
    expect(gauge?.getAttribute("aria-valuetext")).toContain("明天 7 点起");
    expect(gauge?.textContent).toContain("明天 7 点起");
    expect(gauge?.textContent).not.toContain("100%");
  });

  it("does not invent a percentage or progressbar for an unlimited allowance", () => {
    render(<StatRows total={{ label: "全站今日已用", used: 123, cap: 0 }} v5={v5} />, host);
    expect(host.querySelector(".stat--v45[role=progressbar]")).toBeNull();
    expect(host.querySelector(".stat--v45.lp-meter")?.textContent).toContain("V4.5123张已出 · 不限量");
    expect(host.querySelector(".lp-site-gauge__fill")).toBeNull();
  });

  it("signals a low remaining allowance in text as well as color", () => {
    render(<StatRows total={{ label: "全站今日已用", used: 480, cap: 500 }} v5={v5} />, host);
    expect(host.querySelector(".stat--v45.lp-meter")?.textContent).toContain("仅剩 4%");
  });

  it("retains the paused V5 state beside the site allowance", () => {
    render(
      <StatRows
        total={{ label: "全站今日已用", used: 100, cap: 500 }}
        v5={{ label: "V5 暂停中", used: 10, cap: 60, paused: true, short: "暂停中" }}
      />,
      host,
    );
    expect(host.querySelector(".lp-meter--paused")?.textContent).toContain("V5暂停中");
    expect(host.querySelector('.lp-meter--paused[role="progressbar"]')?.getAttribute("aria-valuenow")).toBe(
      "0",
    );
  });
});

it("shares one scale and origin, with V5 as a prefix of total usage", () => {
  render(
    <StatRows
      total={{ label: "全站今日已用", used: 100, cap: 500 }}
      v5={{ label: "V5 已用", used: 20, cap: 60 }}
    />,
    host,
  );
  const all = host.querySelector("path.stat--v45");
  const subset = host.querySelector("path.stat--v5");
  expect(all?.getAttribute("stroke-dasharray")).toBe("20 100");
  expect(subset?.getAttribute("stroke-dasharray")).toBe("4 100");
  expect(subset?.getAttribute("d")).toBe(all?.getAttribute("d"));
  expect(subset?.getAttribute("stroke-dashoffset")).toBeNull();
  expect(host.textContent).not.toMatch(/已用 \d+%/);
});

it("puts a first V5 image at the common start without inventing V4.5 usage", () => {
  render(
    <StatRows
      total={{ label: "全站今日已用", used: 1, cap: 100 }}
      v5={{ label: "V5 已用", used: 1, cap: 20 }}
    />,
    host,
  );
  expect(host.querySelector("path.stat--v5")?.getAttribute("stroke-dasharray")).toBe("1 100");
  expect(host.querySelector("path.stat--v45")?.getAttribute("stroke-dasharray")).toBe("1 100");
  expect(host.querySelector("svg title")?.textContent).toContain("今日总量 1 张，V5 1 张");
});

it("retains actual generation history when a paused or next-day quota hides its usage", () => {
  render(
    <StatRows
      total={{ label: "全站明天可用（明天 7 点起）", used: 0, cap: 100 }}
      v5={{ label: "V5 暂停中", used: 0, cap: 20, paused: true, short: "暂停中" }}
      generated={{ total: 100, v5: 20 }}
    />,
    host,
  );
  expect(host.querySelector("path.stat--v5")?.getAttribute("stroke-dasharray")).toBe("20 100");
  expect(host.textContent).not.toMatch(/已用 \d+%/);
  expect(host.textContent).toContain("明天 7 点起");
  expect(host.textContent).toContain("暂停中");
});
