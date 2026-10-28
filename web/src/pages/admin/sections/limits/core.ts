import type { FieldSpec } from "../system/core";
export type Settings = {
  budgets: { daily_v5: number; daily_anlas: number; monthly_anlas: number };
  v5_paused: boolean;
  v5_medium_mode: "off" | "on";
};

/** /admin/api/runtime-limits 的范围（与后端 RUNTIME_LIMIT_BOUNDS 一致）；全部手动 */
export const RUNTIME_SPECS: Record<string, FieldSpec> = {
  queue_timeout: { kind: "int", label: "排队超时", min: 15, max: 300, unit: "秒", hint: "最多排多久" },
  image_429_cooldown_seconds: {
    kind: "int",
    label: "429 冷却",
    min: 60,
    max: 3600,
    unit: "秒",
    hint: "上游返回 429 后，全站暂停出图多久（上游要求更久时按上游；那个账号另按上游要求单独冷却）",
  },
};

/** /admin/api/guard */
export type GuardField = { default: number; min: number; max: number; label: string };
export type GuardAccount = {
  position: number;
  usable: boolean;
  /** 今天出满了（单号每日上限）：账号本身可用，但今天不再接图 */
  full?: boolean;
  today: number;
  /** 两次开始出图的平均间距（秒）：出图耗时 + 出完后平均歇的秒数 */
  cycle?: number;
  last_429?: number | null;
  cooling?: number;
  locked_today?: number;
  waiting?: number;
  queue_cap?: number | null;
};
export type Guard = {
  values: Record<string, number>;
  fields: Record<string, GuardField>;
  accounts?: GuardAccount[];
  queue_cap?: number | null;
};

/** 账号保护字段的短标题 / 说明（接口的 label 是整段说明文字）。 */
export const GUARD_COPY: Record<string, { label: string; hint: string; unit?: string }> = {
  account_daily_cap: { label: "单号每天上限", hint: "每个上游账号每天最多出几张；0 = 不限", unit: "张" },
  post_jitter_min: {
    label: "每张出完后歇（最少）",
    hint: "这个账号每张出完后歇 N～M 秒（随机）再接下一张；至少 1 秒",
    unit: "秒",
  },
  post_jitter_max: { label: "每张出完后歇（最多）", hint: "小于最少时按最少", unit: "秒" },
  queue_per_member: { label: "每人排队", hint: "每个成员最多同时排几张", unit: "张" },
  queue_per_account: { label: "账号排队", hint: "每个账号全站最多同时排几张", unit: "张" },
};

/** 接口给的字段 → 表单规格（范围取自接口） */
export function guardSpecs(g: Guard): Record<string, FieldSpec> {
  const out: Record<string, FieldSpec> = {};
  for (const [name, f] of Object.entries(g.fields)) {
    const copy = GUARD_COPY[name];
    out[name] = {
      kind: "int",
      label: copy?.label ?? f.label,
      min: f.min,
      max: f.max,
      hint: `${copy?.hint ?? f.label}（${f.min}～${f.max}，默认 ${f.default}）`,
      ...(copy?.unit ? { unit: copy.unit } : {}),
    };
  }
  return out;
}

/** /admin/api/quota-records：只显示 */
/** quiet_rate：该账号自己的实测恢复（快照分账号估算；null = 数据不够） */
export type RecordAccount = {
  position: number;
  v5_percent: number | null;
  v5_recharge_per_day: number | null;
  anlas: number | null;
  quiet_rate: { rate: number; hours: number } | null;
  today: { images: number; v5: number };
  yesterday: { images: number; v5: number };
};
export type Records = {
  accounts: RecordAccount[];
  manual: {
    account_caps: Array<{ position: number | null; cap: number; until: number }>;
    paid_v5: Array<{ position: number | null; max_anlas: number; spent: number; until: number }>;
  };
};

/** 「3 分钟前」「2 小时前」；没有为「没有」 */
export function agoText(ts: number | null | undefined, nowMs: number): string {
  if (!ts) return "没有";
  const s = Math.max(0, Math.round(nowMs / 1000 - ts));
  if (s < 60) return `${s} 秒前`;
  if (s < 3600) return `${Math.floor(s / 60)} 分钟前`;
  if (s < 86400) return `${Math.floor(s / 3600)} 小时前`;
  return `${Math.floor(s / 86400)} 天前`;
}

/** 空值显示「—」 */
export function dash(v: number | null | undefined, unit = ""): string {
  return v == null ? "—" : `${v}${unit}`;
}
