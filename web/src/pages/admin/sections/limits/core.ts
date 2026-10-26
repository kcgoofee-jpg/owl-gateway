/** 额度与保护：接口类型与字段规格（纯函数，可单测）。
 * 站长 10-23：所有自动额度算法删除，这一页上的每个数都是站长手动设的；「记录」只显示，不参与任何限额。 */
import type { FieldSpec } from "../system/core";

/** /admin/api/settings */
export type Settings = {
  register_daily_images: number;
  register_daily_v5: number;
  global_daily_v5: number;
  v5_paused: boolean;
  member_daily_anlas: number;
  global_daily_anlas: number;
  global_monthly_anlas: number;
  quota_modes: GlobalQuotaModes;
  v5_medium_mode: string;
  v5_medium_state?: { active: boolean };
  v5_access_mode: "legacy" | "closed" | "restricted" | "everyone";
  v5_authorized_discord_ids: string[];
  gateway_owner_discord_ids: string[];
  gateway_admin_discord_ids: string[];
};

/**
 * 10-27 规范化：全局额度只有数字一种形态（0 = 停用，上限 9999 / 每月池 99999），没有「不限」。
 * `unlimited` 分支保留只为兼容迁移前的旧接口数据，UI 不再产生它。
 */
export type QuotaMode = { mode: "unlimited" } | { mode: "limit"; value: number };
/** 全局额度数字的统一上限（与后端 DAILY_CAP_MAX / MONTHLY_CAP_MAX 一致） */
export const DAILY_CAP_MAX = 9999;
export const MONTHLY_CAP_MAX = 99999;
export type GlobalQuotaModes = {
  register_daily_images: QuotaMode;
  register_daily_v5: QuotaMode;
  global_daily_v5: QuotaMode;
  global_daily_anlas: QuotaMode;
  member_daily_anlas: QuotaMode;
  global_monthly_anlas: QuotaMode;
  public_vibe_enabled: boolean;
};

/** /admin/api/settings 的数字字段（范围与后端 put_settings 一致），全部手动 */
export const QUOTA_SPECS: Record<string, FieldSpec> = {
  register_daily_images: {
    kind: "int",
    label: "V4.5 每人每天",
    min: 0,
    max: 9999,
    unit: "张",
    hint: "统一额度；0 = 停用 V4.5",
  },
  register_daily_v5: {
    kind: "int",
    label: "V5 每人每天",
    min: 0,
    max: 9999,
    unit: "张",
    hint: "统一额度；0 = 停用 V5",
  },
  global_daily_v5: {
    kind: "int",
    label: "V5 全站每天",
    min: 0,
    max: 9999,
    unit: "张",
    hint: "全站日上限，不是官方余量；0 = 停用全站 V5",
  },
  member_daily_anlas: {
    kind: "int",
    label: "Anlas 每人每天",
    unit: "Anlas",
    min: 0,
    max: 9999,
    hint: "个人默认日上限，独立额度优先；0 = 停用",
  },
  global_daily_anlas: {
    kind: "int",
    label: "Anlas 全站每天",
    unit: "Anlas",
    min: 0,
    max: 9999,
    hint: "全站 Anlas 日池；0 = 停用（免费 Key 的 Vibe / 精确参考随之关闭，付费 Key 不受限）",
  },
  global_monthly_anlas: {
    kind: "float",
    label: "Anlas 全站每月",
    unit: "Anlas",
    min: 0,
    max: 99999,
    hint: "全站月度 Anlas 消耗上限；0 = 停用",
  },
  v5_medium_mode: {
    kind: "select",
    label: "V5 Full 转为 Medium",
    options: [
      { value: "off", label: "关闭：保留请求模型" },
      { value: "on", label: "开启：Full 及其局部重绘转为 Medium" },
    ],
  },
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
