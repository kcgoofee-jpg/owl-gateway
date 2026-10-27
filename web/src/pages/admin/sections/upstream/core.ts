/** 上游：接口类型与文案（纯函数，可单测）。数据来自聚合接口 GET /admin/api/upstream，一页一次取全。 */
import type { FieldSpec } from "../system/core";

/** 一行官方余量读数（/user/subscription 的缓存，只有真实出图或定时查询会刷新）。 */
export type V5Row = {
  account: number;
  percent: number | null;
  time_until_next_percent_seconds: number | null;
  current_response_recharge_per_day_estimate: number | null;
  historical_sample_based_recharge_per_day_estimate: number | null;
  active: boolean | null;
  tier: number | null;
  expires_at: number | null;
  is_grace_period: boolean | null;
  account_type: number | null;
  purchased_percent: number | null;
  fixed_anlas: number | null;
  purchased_anlas: number | null;
  is_negative: boolean | null;
  checked_at: number | null;
  low: boolean;
  uncertain: boolean;
  error: string | null;
  stale: boolean;
};

/** 一行账号状态（/user/information 的缓存，手动查询时才刷新；只保存运维字段，原始响应不落库）。 */
export type InfoRow = {
  token_id: string;
  account: number;
  ban_status: string | null;
  ban_message: string | null;
  email_verified: boolean | null;
  account_created_at: number | null;
  trial_activated: boolean | null;
  trial_actions_left: number | null;
  trial_images_left: number | null;
  last_success: number | null;
  last_error: string | null;
  last_error_at: number | null;
  stale: boolean;
};

/** 一个时间窗内某个账号的请求统计。 */
export type WindowStats = {
  ok: number;
  error: number;
  rejected: number;
  up_429: number;
  up_403: number;
  up_401: number;
};

/** 由对账快照 + 之后的账本用量推算的 Anlas 余额。 */
export type AnlasEstimate = {
  cycle_rolled: boolean;
  balance: number | null;
  fixed: number | null;
  purchased: number | null;
  expires_at: number | null;
  days_left: number | null;
  avg_daily: number;
  projection: "unknown" | "runs_out" | "waste";
  projection_value: number | null;
  snapshot_age_days: number;
  stale: boolean;
  approximate: boolean;
  unattributed_anlas: number;
};

/** 「上游」页的一行账号：令牌池状态 + 官方读数 + 本站观测，按 token_id 合并。 */
export type UpstreamAccount = {
  token_id: string;
  position: number;
  /** 打码后的 Token（完整 Token 从不出站） */
  token: string;
  usable: boolean;
  /** 今天出满了（单号每日上限）：账号本身可用，但今天不再接图 */
  full?: boolean;
  admin_enabled: boolean;
  disabled: boolean;
  fails: number;
  blocked_for: number;
  last_ok: number;
  allow_anlas: boolean;
  image_concurrency: number;
  image_active: number;
  v5_daily_limit: number;
  v5_used: number;
  images_today: number;
  /** 本站观测：请求窗口与负载占比 */
  w1h: WindowStats;
  w24h: WindowStats;
  load_share_24h: number | null;
  balance_share: number | null;
  /** Anlas 核对推算（没有核对快照时为 null） */
  balance: AnlasEstimate | null;
  /** V5 官方余量（/user/subscription 缓存） */
  v5: V5Row | null;
  /** 账号状态（/user/information 缓存） */
  info: InfoRow | null;
};

export type UpstreamPage = {
  generated_at: number;
  pool_configured: boolean;
  accounts: UpstreamAccount[];
  threshold: number;
  reconciliation: Recon;
  settlements: Settlement[];
  info_refresh: { last_attempt_at: number | null; cooldown_seconds: number; retry_after: number };
  /** 核对（POST /admin/api/reconciliation）与释放未知预留共用的会话 CSRF */
  csrf_token: string;
  /** 手动查官方信息（POST /admin/api/upstream/refresh-info）的会话 CSRF */
  csrf_info: string;
};

export type TokenState = { text: string; tone: "ok" | "warn" | "danger" | undefined };
export function tokenState(
  p: Pick<UpstreamAccount, "admin_enabled" | "disabled" | "blocked_for" | "full">,
): TokenState {
  if (!p.admin_enabled) return { text: "已停用", tone: undefined };
  if (p.disabled) return { text: "已失效（401）", tone: "danger" };
  if (p.blocked_for > 0) return { text: `限流中 ${p.blocked_for} 秒`, tone: "warn" };
  if (p.full) return { text: "今日已满", tone: "warn" };
  return { text: "正常", tone: "ok" };
}

export const TOKEN_SPECS: Record<string, FieldSpec> = {
  v5_daily_limit: {
    kind: "int",
    label: "免费 V5 日限",
    min: 0,
    max: 9999,
    unit: "张",
    hint: "0 = 停用这把 Token 的免费 V5；上限 9999",
  },
};

/** 上游 Token 的格式：以 pst- 开头（NovelAI 账号设置 → Get Persistent API Token） */
export function tokenFormatProblem(t: string): string | null {
  const v = t.trim();
  if (!v) return "请粘贴 Token";
  if (!v.startsWith("pst-"))
    return "Token 格式不对：应以 pst- 开头（NovelAI 账号设置 → Get Persistent API Token）";
  return null;
}

export const anlasText = (n: number | null | undefined): string =>
  n == null ? "—" : n.toLocaleString("zh-CN", { maximumFractionDigits: 2 });

export const requestsText = (w: WindowStats): string =>
  `成功 ${w.ok} · 出错 ${w.error} · 选号后被拒 ${w.rejected}`;

export const upstreamErrorsText = (w: WindowStats): string =>
  `429 × ${w.up_429} · 403 × ${w.up_403} · 401 × ${w.up_401}`;

export const daysLeftText = (b: AnlasEstimate): string =>
  b.days_left == null ? "—" : b.days_left < 1 ? "不到 1 天" : `${Math.floor(b.days_left)} 天`;

export const ageText = (days: number): string => (days < 1 ? "今天核对过" : `${Math.round(days)} 天前核对`);

/** Anlas 核对（POST /admin/api/reconciliation）。 */
export type Comparison = {
  reason: string | null;
  since?: number;
  balance_decrease?: number;
  recorded?: number;
  /** 其中账号自己付的站付 V5（不进成员计数） */
  site_paid?: number;
  difference?: number;
  unconfirmed_anlas?: number;
  unconfirmed_requests?: number;
};
export type ReconRecord = {
  id: number;
  checked_at: number;
  accounts: Record<string, unknown>;
  balance: number;
  comparison: Comparison;
};
export type Recon = { history: ReconRecord[]; retry_after: number; running: boolean };

export const RECON_REASONS: Record<string, string> = {
  first: "已记下当前余额",
  accounts_changed: "账号列表有变化，已重新记录余额",
  cycle_changed: "订阅周期已更新，已重新记录余额",
  balance_increased: "余额增加了，已重新记录余额",
  ledger_reset: "本站用量记录有变动，已重新记录余额",
};

/** 待核对的未知请求：上游可能已执行但结果未知，占着额度等站长核对。 */
export type Settlement = {
  request_id: string;
  rid: string;
  key_id: number;
  correlation_id: string;
  status: string;
  service_day: string;
  service_month: string;
  reserved_anlas: number;
  reserved_v5: number;
  reserved_legacy: number;
  is_vibe: boolean;
};
