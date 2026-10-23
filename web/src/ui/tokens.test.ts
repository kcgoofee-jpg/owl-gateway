import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

// 从 tokens.css 读出亮色（:root）与暗色（[data-theme="dark"]）两套颜色，逐对计算 WCAG 对比度。
const css = readFileSync(resolve(import.meta.dirname, "tokens.css"), "utf8");

function block(selector: string): Record<string, string> {
  const start = css.indexOf(selector);
  const open = css.indexOf("{", start);
  const close = css.indexOf("}", open);
  const out: Record<string, string> = {};
  for (const m of css.slice(open + 1, close).matchAll(/--([a-z0-9-]+):\s*(#[0-9a-f]{6})\b/gi)) {
    out[m[1] as string] = (m[2] as string).toLowerCase();
  }
  return out;
}

function luminance(hex: string): number {
  const ch = [1, 3, 5].map((i) => Number.parseInt(hex.slice(i, i + 2), 16) / 255);
  const lin = ch.map((c) => (c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4));
  return 0.2126 * (lin[0] ?? 0) + 0.7152 * (lin[1] ?? 0) + 0.0722 * (lin[2] ?? 0);
}

function contrast(a: string, b: string): number {
  const [x, y] = [luminance(a), luminance(b)].sort((p, q) => q - p) as [number, number];
  return (x + 0.05) / (y + 0.05);
}

const TEXT = ["text", "text-2", "muted", "accent-text", "ok", "warn", "orange", "danger", "info", "v5"];
// 额度类型色（c-*）只画圆环和进度条，不当文字色：按非文字图形的 3:1 查（WCAG 1.4.11），轨道底色也算
const GRAPHICS = ["c-v45", "c-v5", "c-anlas", "c-site", "c-site-v5"];
const GRAPHIC_BACKGROUNDS = ["bg", "surface", "surface-2", "surface-3"];
const BACKGROUNDS = ["bg", "surface", "surface-2"];
const PAIRS: Array<[string, string]> = [
  ["accent-fg", "accent"],
  ["text-2", "surface-3"],
  ["accent-text", "accent-soft"],
  ["ok", "ok-soft"],
  ["warn", "warn-soft"],
  ["danger", "danger-soft"],
  ["info", "info-soft"],
  ["discord-fg", "discord"],
  ["discord-fg", "discord-hover"],
];

const light = block(":root {");
// 暗色块只写和亮色不同的变量（Discord 色两套通用），没写的沿用亮色
const darkOverride = block(':root[data-theme="dark"]');
const dark = { ...light, ...darkOverride };

describe("深色变量两处一致", () => {
  it("跟随系统的深色块和手动深色块必须是同一套", () => {
    const media = block(':root:not([data-theme="light"])');
    expect(Object.keys(media).length).toBeGreaterThan(0);
    expect(media).toEqual(darkOverride);
  });
});

describe.each([
  ["亮色", light],
  ["暗色", dark],
])("%s主题对比度（文字 ≥ 4.5:1）", (_name, t) => {
  for (const fg of TEXT) {
    for (const bg of BACKGROUNDS) {
      it(`${fg} on ${bg}`, () => {
        expect(t[fg], `缺少 --${fg}`).toBeDefined();
        expect(contrast(t[fg] as string, t[bg] as string)).toBeGreaterThanOrEqual(4.5);
      });
    }
  }
  for (const fg of GRAPHICS) {
    for (const bg of GRAPHIC_BACKGROUNDS) {
      it(`${fg} on ${bg}（图形 3:1）`, () => {
        expect(t[fg], `缺少 --${fg}`).toBeDefined();
        expect(contrast(t[fg] as string, t[bg] as string)).toBeGreaterThanOrEqual(3);
      });
    }
  }
  for (const [fg, bg] of PAIRS) {
    it(`${fg} on ${bg}`, () => {
      expect(contrast(t[fg] as string, t[bg] as string)).toBeGreaterThanOrEqual(4.5);
    });
  }
});
