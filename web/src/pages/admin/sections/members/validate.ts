/** 表单校验：与后端 num_int / num_float（crates/gate-server/src/routes/admin）同一套规则（空值、负数、小数、越界一律拒绝，绝不静默变成 0）。 */
export type NumResult = { ok: true; value: number } | { ok: false; error: string };

export function parseNum(
  raw: string,
  label: string,
  o: { min?: number; max: number; int?: boolean },
): NumResult {
  const min = o.min ?? 0;
  if (raw == null || !String(raw).trim()) return { ok: false, error: `${label}不能为空` };
  const v = Number(raw);
  if (!Number.isFinite(v)) return { ok: false, error: `${label}必须是有效数字` };
  if (o.int !== false && !Number.isInteger(v)) return { ok: false, error: `${label}必须是整数` };
  if (v < min || v > o.max) return { ok: false, error: `${label}必须在 ${min}～${o.max} 之间` };
  return { ok: true, value: v };
}

/** Discord 用户 ID：17～20 位纯数字。 */
export const isDiscordId = (s: string): boolean => /^\d{17,20}$/.test(s.trim());
