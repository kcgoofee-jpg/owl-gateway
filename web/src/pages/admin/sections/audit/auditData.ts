/** 生成记录的纯函数：解析 extra、参数摘要、画廊缩略图比例、提示词复制文本。 */
import { secTime } from "../../../../lib/format";
import type { AuditExtra, AuditItem } from "../logs/types";

export function parseExtra(raw: string | null | undefined): AuditExtra {
  if (!raw) return {};
  try {
    const v = JSON.parse(raw) as unknown;
    return v && typeof v === "object" ? (v as AuditExtra) : {};
  } catch {
    return {};
  }
}

export const hasPicture = (it: Pick<AuditItem, "has_image" | "has_thumb">): boolean =>
  !!(it.has_image || it.has_thumb);

/** 宽 / 高，缺失时按竖图 2:3 */
export function aspectOf(ex: AuditExtra): number {
  const w = Number(ex.params?.width);
  const h = Number(ex.params?.height);
  return w > 0 && h > 0 ? w / h : 2 / 3;
}

/** 采样器枚举 → 显示名。只映射仓库里出现过（测试 / 快照）的值，没见过的原样显示。 */
const SAMPLER_LABELS: Record<string, string> = {
  k_euler: "Euler",
  k_euler_ancestral: "Euler A",
  k_dpmpp_2m: "DPM++ 2M",
};

/** 「种子 123 · Euler · 28 步 · CFG 5 · 832×1216」 */
export function paramsLine(ex: AuditExtra): string {
  const p = ex.params ?? {};
  const size = p.width && p.height ? `${p.width}×${p.height}` : "";
  return [
    p.seed != null ? `种子 ${p.seed}` : "",
    p.sampler ? (SAMPLER_LABELS[p.sampler] ?? p.sampler) : "",
    p.steps ? `${p.steps} 步` : "",
    p.scale != null ? `CFG ${p.scale}` : "",
    size,
  ]
    .filter(Boolean)
    .join(" · ");
}

/** 大图下方的一行：「编号 edae7aa9 · 种子 123 · Euler · 28 步 · CFG 5 · 832×1216 · 10-14 07:00:05 · 成员」 */
export const infoLine = (it: Pick<AuditItem, "rid" | "ts" | "key_name">, ex: AuditExtra): string =>
  [it.rid ? `编号 ${it.rid}` : "", paramsLine(ex), secTime(it.ts), it.key_name || "已删除的 Key"]
    .filter(Boolean)
    .join(" · ");

/** 角色提示词合并成一段 */
export const charsText = (list: string[] | undefined): string =>
  (list ?? []).map((c, i) => `角色${i + 1}：${c}`).join("\n");
/** 「复制正面提示词」：正面 + 角色提示词 */
export const positiveText = (it: Pick<AuditItem, "prompt">, ex: AuditExtra): string =>
  [it.prompt, charsText(ex.char_prompts)].filter(Boolean).join("\n");
export const negativeText = (it: Pick<AuditItem, "negative">, ex: AuditExtra): string =>
  [it.negative, ...(ex.char_negatives ?? []).map((c, i) => `角色${i + 1}负面：${c}`)]
    .filter(Boolean)
    .join("\n");

/** Complete prompt text for one-click copying, including positive/negative role prompts. */
export const allPromptText = (it: Pick<AuditItem, "prompt" | "negative">, ex: AuditExtra): string =>
  [positiveText(it, ex), negativeText(it, ex)].filter(Boolean).join("\n\n");

export const STATUS_TEXT: Record<string, string> = { ok: "成功", rejected: "拒绝", error: "错误" };
export const statusText = (s: string | null | undefined): string => STATUS_TEXT[s ?? ""] ?? (s || "错误");
export const statusTone = (s: string): "ok" | "warn" | "danger" =>
  s === "ok" ? "ok" : s === "rejected" ? "warn" : "danger";
