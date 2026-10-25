/** 上游：接口类型与小工具（纯函数，可单测）。 */
import type { FieldSpec } from "../system/core";

/** /admin/api/overview 里的令牌池（掩码后的 Token，从不含完整 Token） */
export type PoolToken = {
  token_id: string;
  position: number;
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
};
export type PoolData = { pool: PoolToken[]; pool_configured: boolean };

export type TokenState = { text: string; tone: "ok" | "warn" | "danger" | undefined };
export function tokenState(p: PoolToken): TokenState {
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
    max: 100_000,
    unit: "张",
    hint: "0 = 不限；只限制这把上游 Token",
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

/** /admin/api/allowance */
export type AllowanceAccount = {
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
export type Allowance = { threshold: number; accounts: AllowanceAccount[]; mode: string };

/** /admin/api/reconciliation */
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
export type Recon = { history: ReconRecord[]; retry_after: number; running: boolean; csrf_token: string };

export const RECON_REASONS: Record<string, string> = {
  first: "已记下当前余额",
  accounts_changed: "账号列表有变化，已重新记录余额",
  cycle_changed: "订阅周期已更新，已重新记录余额",
  balance_increased: "余额增加了，已重新记录余额",
  ledger_reset: "本站用量记录有变动，已重新记录余额",
};

export const anlasText = (n: number | null | undefined): string =>
  n == null ? "—" : n.toLocaleString("zh-CN", { maximumFractionDigits: 2 });
