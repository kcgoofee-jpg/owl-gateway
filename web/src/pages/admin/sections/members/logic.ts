/**
 * 成员判断逻辑（纯函数，无界面依赖，方便单测；总览的「成员要处理」提醒也用这里的 needsAttention）。
 * 时间单位都是 Unix 秒。
 */
import { fullTime, rel, scopeText } from "../../../../lib/format";
import type { AutoTag, Member, Role, Tag } from "./types";

export const HOUR = 3600;
export const DAY = 86400;
/** 站长点「没问题」后，24 小时内这些提醒不再进「需要处理」 */
export const ACK_TAG = "已查看";

export const userTags = (m: Pick<Member, "tags">): Tag[] => m.tags.filter((t) => t.tag !== ACK_TAG);
export const acked = (m: Pick<Member, "tags">, now: number): boolean =>
  m.tags.some((t) => t.tag === ACK_TAG && now - (t.ts || 0) < DAY);

export const memberLabel = (m: Pick<Member, "discord" | "name">): string =>
  m.discord?.display_name || m.discord?.username || m.name;

export function roleOf(m: Pick<Member, "role" | "tags">): Role {
  if (m.role) return m.role;
  return m.tags.some((t) => t.tag === "邀请") ? "invite" : "regular";
}

/** 成员在「按成员筛选」里的取值：名下所有 Key 的 id 用逗号连起来。 */
export const memberKeyValue = (m: Pick<Member, "id" | "all_key_ids">): string =>
  (m.all_key_ids ?? [m.id]).join(",");

/** 「成员」下拉的选项：多把 Key 时名字后面带「（N 把 Key）」，提示是账号名。 */
export const memberKeyOptions = (
  members: Pick<Member, "id" | "all_key_ids" | "discord" | "name" | "key_count">[],
): { value: string; label: string; hint: string }[] =>
  members.map((m) => ({
    value: memberKeyValue(m),
    label: `${m.discord?.display_name || m.name}${m.key_count > 1 ? `（${m.key_count} 把 Key）` : ""}`,
    hint: m.name,
  }));

/** 时长文字：「5 分钟 / 3 小时 / 2 天」 */
export function ago(sec: number): string {
  if (sec < HOUR) return `${Math.max(1, Math.round(sec / 60))} 分钟`;
  if (sec < DAY) return `${Math.round(sec / HOUR)} 小时`;
  return `${Math.round(sec / DAY)} 天`;
}

/* ---------------- 待处理事项 ---------------- */
export type IssueAction =
  | { kind: "share-clear" }
  | { kind: "edit" }
  | { kind: "ack" }
  | { kind: "tag-del"; tag: string };
export type Issue = {
  id: string;
  level: "bad" | "warn" | "info";
  title: string;
  detail: string;
  /** true = 需要站长看一眼（可以点「没问题」24 小时内不再提醒） */
  need?: boolean;
  actions: IssueAction[];
};

export function memberIssues(m: Member, now: number): Issue[] {
  const out: Issue[] = [];
  if (m.share && m.share.paused_until > now) {
    out.push({
      id: "share-paused",
      level: "bad",
      title: "Key 已被限流暂停",
      detail: `到 ${fullTime(m.share.paused_until)} 自动恢复（多半是客户端死循环重试触发）`,
      actions: [{ kind: "share-clear" }],
    });
  }
  if (m.expires_at && m.expires_at < now) {
    out.push({
      id: "expired",
      level: "bad",
      title: "Key 已过期",
      detail: "成员现在用不了",
      actions: [{ kind: "edit" }],
    });
  } else if (m.expires_at && m.expires_at - now < 3 * DAY) {
    out.push({
      id: "expiring",
      level: "warn",
      title: `${Math.max(1, Math.ceil((m.expires_at - now) / DAY))} 天后到期`,
      detail: fullTime(m.expires_at),
      need: true,
      actions: [{ kind: "edit" }, { kind: "ack" }],
    });
  }
  // 自动标签：单纯客户端短时连点「请求过快」不升级为要处理
  for (const t of m.auto_tags as AutoTag[]) {
    const rateLimit = t.tag === "请求过快";
    const loud = !rateLimit && (!!t.note || t.count >= 50);
    out.push({
      id: `auto:${t.tag}`,
      level: loud ? "warn" : "info",
      title: t.tag,
      detail:
        t.note ||
        `近 24 小时因此被拒 ${t.count} 次。是客户端设置的问题，本人改了就好${loud ? "；次数多，可能是客户端在自动重试" : ""}`,
      need: loud,
      actions: loud ? [{ kind: "ack" }] : [],
    });
  }
  for (const t of userTags(m).filter((x) => /疑似|待查|观察/.test(x.tag))) {
    out.push({
      id: `tag:${t.tag}`,
      level: "warn",
      title: `站长标记：${t.tag}`,
      detail: `${t.note || ""}${t.ts ? ` · ${rel(t.ts, now)}` : ""}`,
      need: true,
      actions: [{ kind: "tag-del", tag: t.tag }],
    });
  }
  if (m.reset_today && m.reset_today.count >= 2) {
    out.push({
      id: "reset-many",
      level: "warn",
      title: `今天被手动重置额度 ${m.reset_today.count} 次`,
      detail: "操作日志里能看到是谁重置的",
      need: true,
      actions: [{ kind: "ack" }],
    });
  }
  // 点过「没问题」的 24 小时内，这些可确认的提醒不再出现（bad 级别要单独处理，不受影响）
  return acked(m, now) ? out.filter((x) => !x.need || x.level === "bad") : out;
}

export const needsAttention = (m: Member, now: number): boolean =>
  memberIssues(m, now).some((x) => x.level === "bad" || (x.need && !acked(m, now)));

/* ---------------- 一句话结论与列表状态 ---------------- */
export function memberVerdict(m: Member, now: number): { title: string; sub: string } {
  const issues = memberIssues(m, now);
  const cap = m.daily_images;
  const used = m.today.images;
  let title: string;
  const bad = issues.find((x) => x.level === "bad");
  if (bad) title = bad.title;
  else if (issues.some((x) => x.need) && !acked(m, now)) {
    title = `有 ${issues.filter((x) => x.need).length} 件事要看一眼`;
  } else if (!m.first_image_at) {
    title = `领了 Key ${m.registered_at ? `${ago(now - m.registered_at)}了` : ""}，还没出过图`;
  } else if (cap > 0 && used >= cap) title = `今天的额度用完了（${used} 张）`;
  else if (used > 0) title = `正常使用，今天出了 ${used} 张`;
  else title = `今天还没用${m.last_used_at ? `，上次是 ${rel(m.last_used_at, now)}` : ""}`;
  const why = (m.rejected_why || []).map(([r, n]) => `${r} ${n}`).join("、");
  const ack = m.tags.find((t) => t.tag === ACK_TAG);
  const sub = [
    m.rejected_24h ? `24 小时被拒 ${m.rejected_24h} 次${why ? `：${why}` : ""}` : "",
    m.reset_today?.count ? `站长今天重置过 ${m.reset_today.count} 次额度` : "",
    acked(m, now) && ack ? `${rel(ack.ts, now)}点过「没问题」` : "",
  ]
    .filter(Boolean)
    .join(" · ");
  return { title, sub };
}

export type StatusBadge = {
  tone: "danger" | "warn" | "info" | "accent" | undefined;
  text: string;
  title?: string;
};
/** 列表里每行只放一个最重要的状态 */
export function memberStatus(m: Member, now: number): StatusBadge | null {
  if (m.share && m.share.paused_until > now) return { tone: "danger", text: "已限流" };
  if (m.expires_at && m.expires_at < now) return { tone: "danger", text: "已过期" };
  if (needsAttention(m, now)) {
    return {
      tone: "warn",
      text: "要处理",
      title: memberIssues(m, now)
        .map((x) => x.title)
        .join("、"),
    };
  }
  if (m.daily_images > 0 && m.today.images >= m.daily_images) return { tone: undefined, text: "额度用完" };
  if (m.registered_at && now - m.registered_at < DAY) return { tone: "info", text: "新加入" };
  if (!m.first_image_at) return { tone: undefined, text: "还没出过图" };
  return null;
}

/* ---------------- 筛选 / 搜索 / 排序 ---------------- */
export type FilterDef = {
  id: string;
  label: string;
  /** 鼠标悬停的完整说明（标签里放不下的口径） */
  hint?: string;
  test: (m: Member) => boolean;
};
/** 横栏里的筛选标签 */
export type FilterSet = { chips: FilterDef[] };
/** 筛选栏的「全部」：没有筛选 */
export const ALL_FILTER = "all";
/** 「24 小时被拒过」：选中后，横栏里紧跟着出现被拒原因 */
export const REJECTED_FILTER = "rejected";

const collator = new Intl.Collator(["zh-Hans-CN", "en"], { numeric: true, sensitivity: "base" });
export const compareText = (a: string, b: string): number => collator.compare(a, b);

/**
 * 一条横栏的全部筛选，按「站长日常最常看」排序。
 * idleDays：后端「满 N 天未生图」回收的天数；0 = 回收关闭，不出现「快被回收」。
 */
export function buildFilters(now: number, idleDays: number): FilterSet {
  const chips: FilterDef[] = [
    { id: ALL_FILTER, label: "全部成员", test: () => true },
    { id: "today", label: "今日活跃", test: (m) => m.today.requests > 0 },
    { id: "never", label: "还没出过图", test: (m) => !m.first_image_at },
    {
      id: "new",
      label: "24 小时新加入",
      test: (m) => !!m.registered_at && now - m.registered_at < DAY,
    },
    { id: REJECTED_FILTER, label: "24 小时被拒过", test: (m) => m.rejected_24h > 0 },
  ];
  if (idleDays > 0) {
    chips.push({
      id: "reclaim",
      label: `快被回收（满 ${idleDays} 天）`,
      hint: `满 ${idleDays} 天没生过图就会被自动删除（不拉黑，之后可以再领）`,
      // 与后端回收口径一致：只看最近一次成功生图（没生过图的从创建时间算），邀请 Key 豁免
      test: (m) => roleOf(m) === "regular" && now - (m.last_image_at ?? m.created_at) > (idleDays - 1) * DAY,
    });
  }
  chips.push(
    {
      id: "invite",
      label: "邀请 Key",
      hint: "带「邀请」标签的 Key（站长发出的邀请，不参与自动回收）",
      test: (m) => roleOf(m) === "invite",
    },
    { id: "attention", label: "需要处理", test: (m) => needsAttention(m, now) },
  );
  return { chips };
}

/** 老链接 / 老书签里的筛选 id 换成现在的 */
export const normalizeFilter = (id: string | null | undefined): string | null =>
  id === "role:invite" ? "invite" : (id ?? null);

export const countFilter = (rows: Member[], f: FilterDef): number => rows.filter(f.test).length;

/** 近 24 小时被拒的原因（请求过快、角色超 6 个…，来自 auto_tags）及人数，多的在前；只数成员，口径与「24 小时被拒过」一致 */
export function rejectReasons(rows: Member[]): Array<{ tag: string; count: number }> {
  const n = new Map<string, number>();
  for (const m of rows) {
    if (m.rejected_24h <= 0) continue;
    for (const t of new Set(m.auto_tags.map((x) => x.tag))) n.set(t, (n.get(t) ?? 0) + 1);
  }
  return [...n]
    .map(([tag, count]) => ({ tag, count }))
    .sort((a, b) => b.count - a.count || compareText(a.tag, b.tag));
}
export const hasReason = (m: Member, tag: string): boolean => m.auto_tags.some((x) => x.tag === tag);

/** 默认筛选：有要处理的人就直接看「需要处理」，否则「全部成员」 */
export function defaultFilter(everyone: Member[], now: number): string {
  return everyone.some((m) => needsAttention(m, now)) ? "attention" : ALL_FILTER;
}

export function matchesQuery(m: Member, raw: string): boolean {
  const q = raw.trim().toLowerCase();
  if (!q) return true;
  return (
    m.name.toLowerCase().includes(q) ||
    String(m.discord_id || "").includes(q) ||
    [m.discord?.username, m.discord?.display_name].some((v) =>
      String(v || "")
        .toLowerCase()
        .includes(q),
    ) ||
    m.tags.some((t) => `${t.tag} ${t.note || ""}`.toLowerCase().includes(q))
  );
}

/** 排序只留表头点击这一处（换向也在表头）；数字列大的在前，同值按名称正序 */
export type SortKey = "today" | "week" | "total" | "recent";

const sortValue: Record<SortKey, (m: Member) => number> = {
  today: (m) => m.today.images,
  week: (m) => m.week.images,
  total: (m) => m.total_images,
  recent: (m) => m.last_used_at || 0,
};

const titleOf = (m: Member): string => memberLabel(m) || "";
export function sortMembers<T extends Member>(list: T[], key: SortKey, dir: "asc" | "desc"): T[] {
  const sign = dir === "asc" ? 1 : -1;
  return list.slice().sort((a, b) => {
    const f = sortValue[key];
    return sign * (f(a) - f(b)) || compareText(titleOf(a), titleOf(b));
  });
}

/** 在成员列表里找到某个 Key（或合并后的任一把 Key）对应的成员 */
export const findByKeyId = <T extends Member>(list: T[], id: number): T | undefined =>
  list.find((m) => m.id === id || (m.all_key_ids || []).includes(id));

export function scopeTextOf(s: { v5: boolean; anlas: boolean }): string {
  return scopeText(`v45${s.v5 ? ",v5" : ""}${s.anlas ? ",anlas" : ""}`);
}
