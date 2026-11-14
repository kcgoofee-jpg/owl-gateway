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
    hint: "全站都不可用时暂停多久；单号 429 只冷却该号，自动改派",
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

/** 账号保护字段的补充说明（标题与范围都来自接口，这里只补单位与一句人话）。 */
export const GUARD_HINTS: Record<string, { hint: string; unit?: string }> = {
  account_daily_cap: {
    hint: "0 = 今天不接单，{max} = 不限",
    unit: "张",
  },
  post_jitter_min: { hint: "随机；拼车用自己的等待设置", unit: "秒" },
  post_jitter_max: { hint: "小于最少时按最少", unit: "秒" },
  queue_per_member: { hint: "正在生成的那张不算", unit: "张" },
  queue_per_account: { hint: "0 = 不接图，{max} = 不限", unit: "张" },
};

/** 接口给的字段 → 表单规格（范围取自接口） */
export function guardSpecs(g: Guard): Record<string, FieldSpec> {
  const out: Record<string, FieldSpec> = {};
  for (const [name, f] of Object.entries(g.fields)) {
    const copy = GUARD_HINTS[name];
    const hint = (copy?.hint ?? "").replaceAll("{max}", String(f.max));
    out[name] = {
      kind: "int",
      label: f.label,
      min: f.min,
      max: f.max,
      hint: `${hint}（${f.min}～${f.max}，默认 ${f.default}）`,
      ...(copy?.unit ? { unit: copy.unit } : {}),
    };
  }
  return out;
}
