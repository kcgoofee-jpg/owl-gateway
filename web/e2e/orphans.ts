import type { Page } from "@playwright/test";

/**
 * 找出「最后一行只剩一两个字」的文字块（站长 10-15：不能出现单个或几个字母 / 汉字单独换行）。
 *
 * 以「行内格式化上下文」为单位：一个块元素里、中间只隔着行内元素的所有文字节点算一段；
 * flex / grid 容器里的文字各自成一个匿名项，单独算。用 Range 取每段文字的行矩形，
 * 按竖直方向重叠归成行；折成两行以上、且最后一行宽度不到 2.2 个字宽的，就算孤字。
 * 返回「文字开头…」列表，空数组 = 通过。
 */
export async function orphans(page: Page): Promise<string[]> {
  return page.evaluate(() => {
    const SKIP = "code, pre, .copy, svg, script, style, [hidden], [aria-hidden='true'], .sr-only";
    const isInline = (el: Element) =>
      getComputedStyle(el).display.startsWith("inline") &&
      getComputedStyle(el).display !== "inline-block" &&
      getComputedStyle(el).display !== "inline-flex" &&
      getComputedStyle(el).display !== "inline-grid";
    // 文字节点 → 它所在的段（最近的非行内祖先；祖先是 flex / grid 时，这个文字节点自己一段）
    const groups = new Map<Element | Text, Text[]>();
    const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
    for (let n = walker.nextNode(); n; n = walker.nextNode()) {
      const t = n as Text;
      if (!(t.textContent ?? "").trim() || !t.parentElement || t.parentElement.closest(SKIP)) continue;
      let host: Element = t.parentElement;
      while (isInline(host) && host.parentElement) host = host.parentElement;
      const d = getComputedStyle(host).display;
      const key: Element | Text = /flex|grid/.test(d) && t.parentElement === host ? t : host;
      const list = groups.get(key) ?? [];
      list.push(t);
      groups.set(key, list);
    }
    const out: string[] = [];
    for (const [key, texts] of groups) {
      const host = key instanceof Text ? (key.parentElement as Element) : key;
      if (getComputedStyle(host).whiteSpace.startsWith("nowrap")) continue;
      const rects: DOMRect[] = [];
      for (const t of texts) {
        const r = document.createRange();
        r.selectNodeContents(t);
        rects.push(...[...r.getClientRects()].filter((x) => x.width > 0.5 && x.height > 0));
      }
      if (rects.length < 2) continue;
      rects.sort((a, b) => a.top - b.top);
      const lines: Array<{ top: number; bottom: number; l: number; r: number }> = [];
      for (const x of rects) {
        const line = lines.find((ln) => x.top < ln.bottom - 2 && x.bottom > ln.top + 2);
        if (line) {
          line.top = Math.min(line.top, x.top);
          line.bottom = Math.max(line.bottom, x.bottom);
          line.l = Math.min(line.l, x.left);
          line.r = Math.max(line.r, x.right);
        } else lines.push({ top: x.top, bottom: x.bottom, l: x.left, r: x.right });
      }
      if (lines.length < 2) continue;
      const last = lines.sort((a, b) => a.top - b.top).at(-1);
      const fs = Number.parseFloat(getComputedStyle(host).fontSize) || 14;
      if (last && last.r - last.l < fs * 2.2)
        out.push(
          texts
            .map((t) => t.textContent)
            .join("")
            .trim()
            .slice(0, 24),
        );
    }
    return out;
  });
}
