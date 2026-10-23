/** 站付 V5 到期提醒（GET /admin/api/sitepaid-hints）：只是建议，开不开由站长手动定。纯函数，方便单测。 */
export type SitepaidHint = {
  token_id: string;
  expires_at: number;
  days_left: number;
  /** 对账推算的固定 Anlas 剩余；旧网关可能没有 */
  fixed?: number;
  stale: boolean;
  manual: true;
  setting_key: string;
  message: string;
};
export type SitepaidHints = {
  generated_at: number;
  hints: SitepaidHint[];
  min_anlas: number;
  within_days: number;
};

/** 预算的默认值：剩余 Anlas 取整（没有就留空让站长填）。 */
export const defaultBudget = (h: SitepaidHint): string =>
  h.fixed != null ? String(Math.floor(h.fixed)) : "";

/** 预算输入 → 正数；不是正数返回 null。 */
export function parseBudget(raw: string): number | null {
  const n = Number(raw.trim());
  return raw.trim() !== "" && Number.isFinite(n) && n > 0 ? n : null;
}

/** POST /admin/api/ops/manual 的请求体：这个号用自己的 Anlas 出 V5，到期时间截止，最多花 budget。 */
export function manualBody(h: SitepaidHint, budget: number, why: string) {
  return {
    key: h.setting_key,
    value: { [h.token_id]: { until: h.expires_at, max_anlas: budget } },
    why,
  };
}
