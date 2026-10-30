// 对比 official vs owl 的 styles.json，输出差异 markdown
import { readFileSync, writeFileSync } from "node:fs";
const base = new URL("./captures/", import.meta.url).pathname;
const off = JSON.parse(readFileSync(base + "official/styles.json", "utf8"));
const owl = JSON.parse(readFileSync(base + "owl/styles.json", "utf8"));
const NUM = new Set(["fontSize", "fontWeight", "lineHeight", "letterSpacing", "gap", "opacity", "width", "height"]);
const px = (v) => parseFloat(v) || 0;
const rows = [];
for (const id of Object.keys(off)) {
  if (id === "__page") continue;
  const a = off[id], b = owl[id];
  if (!a || !b) { rows.push(`| ${id} | — | — | ❌ ${!a ? "官网未命中" : ""}${!b ? "owl 未命中" : ""} |`); continue; }
  const deltas = [];
  for (const k of Object.keys(a)) {
    if (k === "tag" || k === "text") continue;
    const va = a[k], vb = b[k];
    if (va === vb) continue;
    if (NUM.has(k)) {
      if (Math.abs(px(va) - px(vb)) <= (k === "opacity" ? 0.02 : 0.6)) continue;
      deltas.push(`${k}: ${va} → ${vb}`);
    } else if (k === "rect") {
      const d = va.map((v, i) => v - vb[i]);
      if (d.every((x) => Math.abs(x) <= 1)) continue;
      deltas.push(`rect[xywh] Δ ${d.map((x) => (x > 0 ? "+" : "") + Math.round(x)).join(" ")}`);
    } else {
      deltas.push(`${k}: "${String(va).slice(0, 40)}" vs "${String(vb).slice(0, 40)}"`);
    }
  }
  rows.push(`| ${id} | ${(a.text || "").slice(0, 20)} | ${deltas.length ? deltas.join("<br>") : "✅ 一致"} |`);
}
const pageA = off.__page, pageB = owl.__page;
const md = `# diff-harness 报告（official-en vs owl @1440×900）

- 官网 bodyBg: ${pageA.bodyBg} / owl: ${pageB.bodyBg}
- 采集时间: ${new Date().toISOString()}

| 控件 | 官网文本 | 差异（官网值 vs owl 值） |
|---|---|---|
${rows.join("\n")}
`;
writeFileSync(base + "../diff-report.md", md);
console.log(md);
