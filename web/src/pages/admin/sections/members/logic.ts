/**
 * 成员判断逻辑（纯函数，无界面依赖，方便单测；总览的「成员要处理」提醒也用这里的 needsAttention）。
 * 时间单位都是 Unix 秒。
 */
import { fullTime, rel, scopeText } from "../../../../lib/format";
import type { Member, Tag } from "./types";

export const HOUR = 3600;
export const DAY = 86400;
/** 站长点「没问题」后，24 小时内这些提醒不再进「需要处理」 */
export const ACK_TAG = "已查看";

export const userTags = (m: Pick<Member, "tags">): Tag[] => m.tags.filter((t) => t.tag !== ACK_TAG);
export const acked = (m: Pick<Member, "tags">, now: number): boolean =>
  m.tags.some((t) => t.tag === ACK_TAG && now - (t.ts || 0) < DAY);

export const memberLabel = (m: Pick<Member, "discord" | "name">): string =>
  m.discord?.display_name || m.discord?.username || m.name;

/** 成员在「按成员筛选」里的取值：名下所有 Key 的 id 用逗号连起来。 */
export const memberKeyValue = (m: Pick<Member, "id" | "all_key_ids">): string =>
  (m.all_key_ids ?? [m.id]).join(",");

/** 从请求日志跳到这位成员（`open` 可以是成员 id 或名下任一 Key id）。 */
export const memberHref = (id: number): string => `/admin/members?open=${id}`;

/** 从成员跳到他的请求日志。 */
export function memberLogsHref(
  m: Pick<Member, "id" | "all_key_ids">,
  extra: Record<string, string> = {},
): string {
  const qs = new URLSearchParams();
  qs.set("key", (m.all_key_ids ?? [m.id]).join(","));
  for (const [k, v] of Object.entries(extra)) if (v) qs.set(k, v);
  return `/admin/logs/usage?${qs}`;
}

export function memberFromOpen(rows: Member[], open: string | null | undefined): Member | undefined {
  if (!open) return undefined;
  const n = Number(open);
  if (!Number.isInteger(n) || n <= 0) return undefined;
  return rows.find((m) => m.id === n || (m.all_key_ids ?? []).includes(n));
}

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

function issueOf(id: string, m: Member, now: number): Issue | null {
  switch (id) {
    case "paused":
      return {
        id,
        level: "bad",
        title: "Key 已被限流暂停",
        detail: `到 ${fullTime(m.paused_until)} 自动恢复（多半是客户端死循环重试触发）`,
        actions: [{ kind: "share-clear" }],
      };
    case "expired":
      return {
        id,
        level: "bad",
        title: "Key 已过期",
        detail: "成员现在用不了",
        actions: [{ kind: "edit" }],
      };
    case "expiring":
      return {
        id,
        level: "warn",
        title: `${Math.max(1, Math.ceil(((m.expires_at ?? now) - now) / DAY))} 天后到期`,
        detail: fullTime(m.expires_at),
        need: true,
        actions: [{ kind: "edit" }, { kind: "ack" }],
      };
    case "reset_many":
      return {
        id,
        level: "warn",
        title: `今天被手动重置额度 ${m.reset_today?.count ?? 0} 次`,
        detail: "操作日志里能看到是谁重置的",
        need: true,
        actions: [{ kind: "ack" }],
      };
    default:
      return null;
  }
}

/** 文案和按钮。哪些事项要出现由列表上的 `issues` 决定，这里不再判断。 */
export function memberIssues(m: Member, now: number): Issue[] {
  return (m.issues ?? []).flatMap((id) => {
    const issue = issueOf(id, m, now);
    return issue ? [issue] : [];
  });
}

export const needsAttention = (m: Member, _now?: number): boolean => (m.issues ?? []).length > 0;

/* ---------------- 一句话结论与列表状态 ---------------- */
export function memberVerdict(m: Member, now: number): { title: string; sub: string } {
  const issues = memberIssues(m, now);
  const cap = m.policy.effective.daily_images;
  const used = m.today.images;
  let title: string;
  const bad = issues.find((x) => x.level === "bad");
  if (bad) title = bad.title;
  else if (issues.some((x) => x.need)) {
    title = `有 ${issues.filter((x) => x.need).length} 件事要看一眼`;
  } else if (!m.first_image_at) {
    title = `领了 Key ${m.registered_at ? `${ago(now - m.registered_at)}了` : ""}，还没出过图`;
  } else if (cap > 0 && m.today.legacy_free_images >= cap) title = "今天的额度用完了";
  else if (used > 0) title = "正常使用";
  else title = `今天还没用${m.last_used_at ? `，上次是 ${rel(m.last_used_at, now)}` : ""}`;
  const ack = m.tags.find((t) => t.tag === ACK_TAG);
  const sub = [
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
  const ids = m.issues ?? [];
  if (ids.includes("paused")) return { tone: "danger", text: "已限流" };
  if (ids.includes("expired")) return { tone: "danger", text: "已过期" };
  if (needsAttention(m, now)) {
    return {
      tone: "warn",
      text: "要处理",
      title: memberIssues(m, now)
        .map((x) => x.title)
        .join("、"),
    };
  }
  if (m.policy.effective.daily_images > 0 && m.today.legacy_free_images >= m.policy.effective.daily_images)
    return { tone: undefined, text: "额度用完" };
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

const collator = new Intl.Collator(["zh-Hans-CN", "en"], { numeric: true, sensitivity: "base" });
export const compareText = (a: string, b: string): number => collator.compare(a, b);

/**
 * 一条横栏的全部筛选，按「站长日常最常看」排序。
 */
export function buildFilters(now: number): FilterSet {
  const chips: FilterDef[] = [
    { id: ALL_FILTER, label: "全部成员", test: () => true },
    { id: "today", label: "今日活跃", test: (m) => m.today.requests > 0 },
    { id: "never", label: "还没出过图", test: (m) => !m.first_image_at },
    {
      id: "new",
      label: "24 小时新加入",
      test: (m) => !!m.registered_at && now - m.registered_at < DAY,
    },
  ];
  chips.push({ id: "attention", label: "需要处理", test: (m) => needsAttention(m, now) });
  return { chips };
}

/** 老链接 / 老书签里的筛选 id 换成现在的（已删的筛选回到「全部」）。 */
export const normalizeFilter = (id: string | null | undefined): string | null => {
  if (!id) return null;
  if (id === "role:invite" || id === "invite" || id === "rejected") return null;
  return id;
};

export const countFilter = (rows: Member[], f: FilterDef): number => rows.filter(f.test).length;

export function matchesQuery(m: Member, raw: string): boolean {
  const terms = raw.normalize("NFKC").trim().toLowerCase().split(/\s+/).filter(Boolean);
  if (!terms.length) return true;
  const fields = [
    m.name,
    m.discord_id,
    m.discord?.username,
    m.discord?.display_name,
    ...m.tags.flatMap((t) => [t.tag, t.note]),
  ].map((v) =>
    String(v || "")
      .normalize("NFKC")
      .toLowerCase(),
  );
  return terms.every((term) => fields.some((field) => field.includes(term)));
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

export function scopeTextOf(s: { v5: boolean; anlas: boolean }): string {
  return scopeText(`v45${s.v5 ? ",v5" : ""}${s.anlas ? ",anlas" : ""}`);
}
