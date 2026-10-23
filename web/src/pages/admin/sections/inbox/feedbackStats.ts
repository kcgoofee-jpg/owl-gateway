/** 成员反馈的筛选、问卷的统计（纯函数）。 */
import type { FeedbackItem, SurveyRow } from "../logs/types";

/** 按成员（Key 名 / 用户名 / Discord ID）或内容搜索；关键字为空 = 全部。旧版选择题的记录搜的是那一行摘要。 */
export function filterFeedback(items: FeedbackItem[], query: string): FeedbackItem[] {
  const q = query.trim().toLowerCase();
  if (!q) return items;
  return items.filter((it) =>
    [it.key_name, it.username, it.discord_id, it.text].filter(Boolean).join(" ").toLowerCase().includes(q),
  );
}

/**
 * 把文字拆成「前面」和「末尾 n 个字」（按字符，不拆表情 / 生僻字）。末尾那一行（最后一个换行之后）不够 n 个字时不拆：
 * 要么整段本来就短，要么最后一行是成员自己换的行，不替他改。
 */
export function splitTail(text: string, n: number): [string, string] {
  const chars = [...text.slice(text.lastIndexOf("\n") + 1)];
  if (chars.length <= n) return [text, ""];
  const tail = chars.slice(-n).join("");
  return [text.slice(0, text.length - tail.length), tail];
}

/* ---------------- 详细问卷（/survey 网页） ---------------- */
export type SurveyStats = {
  total: number;
  topClient: [string, number] | null;
  topImprove: [string, number] | null;
};

function tally(rows: SurveyRow[], field: "improve"): Array<[string, number]> {
  const o = new Map<string, number>();
  for (const r of rows) {
    for (const part of String(r[field] ?? "").split("、")) {
      const p = part.trim();
      if (p) o.set(p, (o.get(p) ?? 0) + 1);
    }
  }
  return [...o.entries()].sort((a, b) => b[1] - a[1]);
}

/** 客户端是单选的自由文本（不按「、」拆）；忽略大小写和首尾空白，显示第一次出现的写法。 */
function tallyClient(rows: SurveyRow[]): Array<[string, number]> {
  const o = new Map<string, [string, number]>();
  for (const r of rows) {
    const c = (r.client ?? "").trim();
    if (!c) continue;
    const k = c.toLowerCase();
    const cur = o.get(k);
    if (cur) cur[1] += 1;
    else o.set(k, [c, 1]);
  }
  return [...o.values()].sort((a, b) => b[1] - a[1]);
}

export function surveyStats(rows: SurveyRow[]): SurveyStats {
  const imp = tally(rows, "improve");
  const cli = tallyClient(rows);
  return {
    total: rows.length,
    topClient: cli[0] ?? null,
    topImprove: imp[0] ?? null,
  };
}
