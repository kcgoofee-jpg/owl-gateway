// 官网 vs 本地/线上 workbench 控件规格对比（ego-browser nodejs < parity.mjs）
// 用法：TARGET=local|prod OFFICIAL_ONLY=1 ego-browser nodejs < tools/nai-parity/parity.mjs
const fs = await import("node:fs/promises");
const OUT = "/Users/davidzhao/ego/owl-nai-1to1/";
const LOCAL = process.env?.TARGET === "prod" ? "https://gate.davidzhao.top/workbench" : "http://localhost:5174/workbench?preview=1";
const TARGET = process.env?.TARGET === "prod" ? "prod" : "local";

const EXTRACT = () => {
  const R = (el) => {
    const b = el.getBoundingClientRect();
    const s = getComputedStyle(el);
    return {
      x: +b.x.toFixed(1), y: +b.y.toFixed(1), w: +b.width.toFixed(1), h: +b.height.toFixed(1),
      bg: s.backgroundColor, color: s.color, op: s.opacity,
      fs: s.fontSize, fw: s.fontWeight, lh: s.lineHeight, ff: s.fontFamily.split(",")[0].replace(/"/g, ""),
      br: s.borderRadius, pad: s.padding, gap: s.gap, border: s.borderTopWidth,
    };
  };
  const leaves = [...document.querySelectorAll("body *")].filter((e) => e.children.length === 0 && (e.textContent || "").trim());
  const pick = (re, maxH = 120) => {
    const c = leaves.filter((e) => { const b = e.getBoundingClientRect(); return b.width > 0 && b.height > 0 && b.height <= maxH && re.test((e.textContent || "").trim()); });
    c.sort((a, b) => (a.textContent || "").length - (b.textContent || "").length);
    return c[0] || null;
  };
  const anchors = {
    anlas: /^Anlas:/, model: /^Model$/, mode: /^Mode$/, modelValue: /^(V4\.5|V5)/,
    promptTab: /^Prompt$/, ucTab: /^Undesired Content$/, quality: /^Quality Tags:/,
    charTitle: /^Character Prompts$/, position: /^Position$/, refImages: /^Reference Images$/,
    img2img: /^Image2Image$/, imgSettings: /^Image Settings$/, resolution: /^Resolution$/,
    nofImages: /^Number of Images$/, steps: /^Steps$/, guidance: /^Guidance$/, seed: /^Seed$/,
    sampler: /^Sampler$/, generate: /^Generate \d+ Image/, history: /^History/, hero: /^Get Started$/,
  };
  const scrollOf = (el) => { let e = el; while (e && e !== document.body) { const s = getComputedStyle(e); if (/(auto|scroll)/.test(s.overflowY) && e.scrollHeight > e.clientHeight + 8) return e; e = e.parentElement; } return null; };
  const out = { viewport: { w: innerWidth, h: innerHeight }, anchors: {} };
  out.scrollBox = null;
  for (const [k, re] of Object.entries(anchors)) {
    const el = pick(re);
    if (!el) { out.anchors[k] = null; continue; }
    const sc = scrollOf(el);
    if (sc && !out.scrollBox) out.scrollBox = R(sc);
    const top = sc ? sc.getBoundingClientRect().y : 0;
    out.anchors[k] = { text: (el.textContent || "").trim().slice(0, 28), relY: +(el.getBoundingClientRect().y - top).toFixed(1), inScroll: Boolean(sc), ...R(el) };
  }
  // 结构件
  const bySel = (sel) => { const e = document.querySelector(sel); return e ? R(e) : null; };
  out.struct = {
    tabsPill: (() => { const t = pick(/^Prompt$/); const p = t?.parentElement; return p ? R(p) : null; })(),
    genBtn: (() => { const b = [...document.querySelectorAll("button")].find((x) => /^Generate \d+ Image/.test((x.textContent || "").trim())); return b ? R(b) : null; })(),
    sideCol: (() => { const e = [...document.querySelectorAll("div")].find((x) => { const b = x.getBoundingClientRect(); return b.x === 0 && b.width > 340 && b.width < 560 && b.height > 500; }); return e ? R(e) : null; })(),
    canvas: (() => { const e = [...document.querySelectorAll("div")].find((x) => { const b = x.getBoundingClientRect(); const s = getComputedStyle(x); return b.width > 600 && b.x > 380 && b.height > 400 && s.overflowY === "auto"; }); return e ? R(e) : null; })(),
  };
  out.nai = {
    side: bySel(".nai-side"), scroll: bySel(".nai-side__scroll"), navRow: bySel(".nai-nav-row"),
    footer: bySel(".nai-footer"), promptCard: bySel(".nai-card"), quota: bySel(".nai-quota"),
    historyRail: bySel(".nai-history"), root: bySel(".nai-root") ? R(document.querySelector(".nai-root")) : null,
  };
  return out;
};

const task = await taskSpace("owl-nai-parity");
const page = task.page("p1");
async function capture(url, tag) {
  await page.goto(url);
  await page.waitForLoadState("networkidle").catch(() => {});
  await page.waitForTimeout(2600);
  return await page.evaluate(EXTRACT);
}
const off = await capture("https://novelai.net/image", "official");
const owl = await capture(LOCAL, TARGET);
await fs.writeFile(`${OUT}parity-official.json`, JSON.stringify(off, null, 1));
await fs.writeFile(`${OUT}parity-${TARGET}.json`, JSON.stringify(owl, null, 1));
await page.screenshot({ path: `${OUT}parity-${TARGET}.png` });

// 差异表（官网 y 含 52px 支付横幅偏移，offset 项给出）
const banner = off.anchors.steps && owl.anchors.steps ? Math.round(off.anchors.steps.y - owl.anchors.steps.y) : 0;
const lines = [`# parity ${TARGET} vs official (dy=${banner})`, ""];
for (const k of Object.keys(off.anchors)) {
  const a = off.anchors[k], b = owl.anchors[k];
  if (!a || !b) { lines.push(`${k}: official=${a ? "ok" : "MISSING"} owl=${b ? "ok" : "MISSING"}`); continue; }
  const d = [];
  if (Math.abs(a.x - b.x) > 2) d.push(`x ${a.x}→${b.x}`);
  if (Math.abs(a.w - b.w) > 2) d.push(`w ${a.w}→${b.w}`);
  if (a.inScroll && b.inScroll && Math.abs(a.relY - b.relY) > 3) d.push(`relY ${a.relY}→${b.relY}`);
  else if (!a.inScroll && !b.inScroll && Math.abs((a.y - banner) - b.y) > 3) d.push(`y ${a.y}→${b.y}`);
  if (a.fs !== b.fs) d.push(`fs ${a.fs}→${b.fs}`);
  if (a.fw !== b.fw) d.push(`fw ${a.fw}→${b.fw}`);
  if (a.color !== b.color) d.push(`color ${a.color}→${b.color}`);
  if (a.bg !== b.bg) d.push(`bg ${a.bg}→${b.bg}`);
  if (a.br !== b.br) d.push(`br ${a.br}→${b.br}`);
  if (a.pad !== b.pad) d.push(`pad ${a.pad}→${b.pad}`);
  if (a.lh !== b.lh) d.push(`lh ${a.lh}→${b.lh}`);
  lines.push(d.length ? `${k}: ${d.join(" | ")}` : `${k}: ok`);
}
lines.push("", "struct official: " + JSON.stringify(off.struct), "struct owl: " + JSON.stringify(owl.struct));
lines.push("scrollBox official: " + JSON.stringify(off.scrollBox) + " owl: " + JSON.stringify(owl.scrollBox));
lines.push("nai: " + JSON.stringify(owl.nai));
console.log(lines.join("\n"));
await fs.writeFile(`${OUT}parity-report.md`, lines.join("\n"));
await task.finish({ keep: [] });
