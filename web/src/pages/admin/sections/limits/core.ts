import type { FieldSpec } from "../system/core";
export type Bound = { kind: "int" | "float"; min: number; max: number };
export type Settings = {
  budgets: { daily_v5: number; daily_anlas: number; monthly_anlas: number };
  v5_paused: boolean;
  v5_medium_mode: "off" | "on";
  fields: { daily_v5: Bound; daily_anlas: Bound; monthly_anlas: Bound };
};

/** /admin/api/runtime-limits：当前值旁边带 fields（min/max）。标题留在前端。 */
export type RuntimeField = { kind?: string; min: number; max: number };
export type RuntimeLimits = {
  queue_timeout: number;
  image_429_cooldown_seconds: number;
  fields: Record<string, RuntimeField>;
};

const RUNTIME_COPY: Record<string, { label: string; unit: string; hint: string }> = {
  queue_timeout: { label: "排队超时", unit: "秒", hint: "最多排多久" },
  image_429_cooldown_seconds: {
    label: "429 冷却",
    unit: "秒",
    hint: "所有上游账号都不可用时，全站暂停出图多久（单号 429 只冷却该号、自动改派其它健康号；上游要求更久时按上游）",
  },
};

export const RUNTIME_NAMES = Object.keys(RUNTIME_COPY);

export function runtimeSpecs(data: RuntimeLimits): Record<string, FieldSpec> {
  const out: Record<string, FieldSpec> = {};
  for (const [name, copy] of Object.entries(RUNTIME_COPY)) {
    const field = data.fields[name];
    if (!field) continue;
    out[name] = {
      kind: "int",
      label: copy.label,
      min: field.min,
      max: field.max,
      unit: copy.unit,
      hint: copy.hint,
    };
  }
  return out;
}

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
  account_daily_cap: {
    label: "单号每天上限",
    hint: "每个上游账号每天最多出几张；0 = 这个账号今天一张都不出（不是不限，要放开填 {max}）",
    unit: "张",
  },
  post_jitter_min: {
    label: "每张出完后歇（最少）",
    hint: "这个账号每张出完后歇 N～M 秒（随机）再接下一张；至少 1 秒",
    unit: "秒",
  },
  post_jitter_max: { label: "每张出完后歇（最多）", hint: "小于最少时按最少", unit: "秒" },
  queue_per_member: { label: "每人排队", hint: "每个成员最多同时排几张", unit: "张" },
  queue_per_account: {
    label: "账号排队",
    hint: "每个上游账号最多同时排几张；全站上限 = 这个数 × 可用账号数，再按排队超时内能开始的张数收紧",
    unit: "张",
  },
};

/** 接口给的字段 → 表单规格（范围取自接口） */
export function guardSpecs(g: Guard): Record<string, FieldSpec> {
  const out: Record<string, FieldSpec> = {};
  for (const [name, f] of Object.entries(g.fields)) {
    const copy = GUARD_COPY[name];
    const hint = (copy?.hint ?? f.label).replaceAll("{max}", String(f.max));
    out[name] = {
      kind: "int",
      label: copy?.label ?? f.label,
      min: f.min,
      max: f.max,
      hint: `${hint}（${f.min}～${f.max}，默认 ${f.default}）`,
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
