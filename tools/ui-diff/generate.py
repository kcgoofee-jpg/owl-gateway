# -*- coding: utf-8 -*-
"""生成 capture-<target>.mjs：同一控件清单，官网/owl 各跑一份，输出截图+样式 JSON。"""
import json
import pathlib

PROPS = [
    "display", "position", "backgroundColor", "color", "borderRadius", "border",
    "padding", "margin", "fontSize", "fontWeight", "lineHeight", "letterSpacing",
    "boxShadow", "transition", "gap", "opacity", "width", "height", "fontFamily",
]

CONTROLS = [
    {"id": "navrow", "owl": "document.querySelector('.nai-nav-row')",
     "official": "(() => { const s = smallest('span', /^Anlas:/); if (!s) return null; let d = s.closest('div'); for (let i = 0; i < 5 && d; i++) { d = d.parentElement; const r = d.getBoundingClientRect(); if (r.width > 300 && r.height < 90) return d; } return d; })()"},
    {"id": "anlas", "owl": "document.querySelector('.nai-anlas')",
     "official": "(() => { const s = smallest('span', /^Anlas:/); if (!s) return null; let d = s.closest('div'); while (d && d.getBoundingClientRect().width < 60) d = d.parentElement; return d ?? s; })()"},
    {"id": "modelpill", "owl": "document.querySelector('.nai-model-cell .nai-pill')",
     "official": "document.querySelector('[class*=singleValue]')?.closest('div')?.parentElement ?? document.querySelector('[class*=singleValue]')"},
    {"id": "prompttab", "owl": "document.querySelector('.nai-tabs') || document.querySelector('.nai-card-head')",
     "official": "btnExact('Undesired Content')"},
    {"id": "promptbox", "owl": "document.querySelector('textarea[aria-label=提示词]')",
     "official": "document.querySelector('.image-gen textarea, [contenteditable=true]')"},
    {"id": "steps", "owl": "document.querySelector('.nai-params')",
     "official": "smallest('div,span', /^Steps$/)?.closest('div')"},
    {"id": "sampler", "owl": "document.querySelector('.nai-param--sampler')",
     "official": "smallest('div,span', /^Sampler$/)?.closest('div')"},
    {"id": "imagesettings", "owl": "byText('.nai-res-title', /Aspect Ratio|Resolution/)?.closest('.nai-card') || document.querySelector('.nai-card')",
     "official": "smallest('div,span', /^Image Settings$/)?.closest('div').parentElement"},
    {"id": "generate", "owl": "document.querySelector('.nai-generate')",
     "official": "smallest('*', /^Generate [0-9]+ Image/)"},
    {"id": "quota", "owl": "document.querySelector('.nai-quota')",
     "official": "smallest('span,div', /% of Opus Generations remaining/)?.closest('div')"},
    {"id": "rail", "owl": "document.querySelector('.nai-rail')",
     "official": "btnExact('History')?.closest('div').parentElement"},
    {"id": "gear", "owl": "document.querySelector('.nai-foot-icon, .nai-settings')",
     "official": "[...document.querySelectorAll('button')].find(b => /settings/i.test(b.getAttribute('aria-label') || '') && b.getBoundingClientRect().width < 80) ?? null"},
]

TEMPLATE = '''// 自动生成：diff-harness 采集脚本（target=__TARGET__）
import { writeFileSync, mkdirSync } from "node:fs";
const DIR = "__OUT__/captures/__TARGET__";
mkdirSync(DIR, { recursive: true });

const task = await taskSpace("diff-harness 采集：__TARGET__");
const page = task.page("p1");
await page.cdp("Emulation.setDeviceMetricsOverride", { width: 1440, height: 900, deviceScaleFactor: 1, mobile: false });
await page.goto("__URL__");
await page.waitForTimeout(8000);
// 官网：尝试关支付横幅（1440 宽时 X 在右上角）与 cookie 横幅
for (const [x, y] of [[1398, 26], [350, 46], [1420, 668]]) {
  try { await page.mouse.click(x, y, { label: "关横幅" }); } catch {}
  await page.waitForTimeout(600);
}
await page.waitForTimeout(1500);

const data = await page.evaluate(() => {
  const PROPS = __PROPS__;
  const vis = (el) => { const r = el.getBoundingClientRect(); return r.width > 0 && r.height > 0; };
  const btns = [...document.querySelectorAll("button")].filter(vis);
  const btnByText = (t) => btns.find(b => (b.textContent || "").trim() === t);
  const btnExact = (t) => btnByText(t);
  const smallest = (sel, re) => {
    const m = [...document.querySelectorAll(sel)].filter(e => re.test((e.textContent || "").trim()) && vis(e));
    if (!m.length) return null;
    return m.reduce((a, b) => ((a.textContent || "").length <= (b.textContent || "").length ? a : b));
  };
  const byText = (sel, re) => smallest(sel, re);
  const CONTROLS = __CONTROLS__;
  const IDS = __IDS__;
  const pick = (el) => {
    if (!el) return null;
    const r = el.getBoundingClientRect();
    const cs = getComputedStyle(el);
    const o = { rect: [Math.round(r.x), Math.round(r.y), Math.round(r.width), Math.round(r.height)] };
    for (const p of PROPS) o[p] = cs.getPropertyValue(p);
    o.text = (el.textContent || "").trim().replace(/\\s+/g, " ").slice(0, 60);
    o.tag = el.tagName;
    return o;
  };
  const out = {};
  for (let i = 0; i < CONTROLS.length; i++) {
    const id = IDS[i];
    let el = null;
    try { el = eval(CONTROLS[i]); } catch (e) { out[id] = { error: String(e).slice(0, 80) }; continue; }
    out[id] = pick(el);
  }
  const body = getComputedStyle(document.body);
  out.__page = { bodyBg: body.backgroundColor, docW: document.documentElement.scrollWidth, innerW: innerWidth, innerH: innerHeight, url: location.href };
  return out;
});

for (const [id, d] of Object.entries(data)) {
  if (!d || !d.rect) continue;
  const [x, y, w, h] = d.rect;
  const pad = 6;
  const clip = { x: Math.max(0, x - pad), y: Math.max(0, y - pad), width: Math.min(w + pad * 2, 1440), height: Math.min(h + pad * 2, 900), scale: 2 };
  try {
    const shot = await page.cdp("Page.captureScreenshot", { format: "png", clip });
    writeFileSync(`${DIR}/${id}.png`, Buffer.from(shot.data, "base64"));
  } catch (e) { console.log("shot fail", id, String(e).slice(0, 60)); }
}
const full = await page.cdp("Page.captureScreenshot", { format: "png" });
writeFileSync(`${DIR}/__full__.png`, Buffer.from(full.data, "base64"));
writeFileSync(`${DIR}/styles.json`, JSON.stringify(data, null, 1));
console.log("captured __TARGET__:", Object.keys(data).join(","));
await task.finish({ keep: [] });
'''

out = pathlib.Path(__file__).parent
for target, url in [("official", "https://novelai.net/image"), ("owl", "http://localhost:5174/_owl/workbench")]:
    key = "official" if target == "official" else "owl"
    body = (
        TEMPLATE
        .replace("__TARGET__", target)
        .replace("__URL__", url)
        .replace("__OUT__", str(out))
        .replace("__PROPS__", json.dumps(PROPS))
        .replace("__CONTROLS__", json.dumps([c[key] for c in CONTROLS], ensure_ascii=False))
        .replace("__IDS__", json.dumps([c["id"] for c in CONTROLS]))
    )
    (out / f"capture-{target}.mjs").write_text(body)
print("generated:", [p.name for p in out.glob("capture-*.mjs")])
