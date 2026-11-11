/** 日志相关的显示格式化（纯函数）：功能名、耗时。模型短名和交付徽标在 lib/request-label。 */
import { secondsText } from "../../../../lib/format";
import { type Feature, RETIRED } from "../features";

export { deliveryLabel, shortModel } from "../../../../lib/request-label";

/** 已删除的 kind。现行 kind → 功能 id 由 GET /admin/api/features 的 kinds 给出。 */
const RETIRED_KIND: Record<string, string> = {
  text: "text",
  chat: "text",
  voice: "voice",
};

export function kindLabel(
  kind: string | null | undefined,
  features: Feature[] = [],
  kinds: Record<string, string> = {},
): string {
  const id = kinds[kind ?? ""] ?? RETIRED_KIND[kind ?? ""];
  const f = features.find((x) => x.id === id) ?? RETIRED.find((x) => x.id === id);
  return f ? f.short : kind || "—";
}

/** 耗时：生成 4.2 秒、等 12 秒（等待 < 100ms 不显示） */
export function timing(waitMs: number, durMs: number): { gen: string; wait: string } {
  return { gen: durMs ? secondsText(durMs) : "", wait: waitMs >= 100 ? secondsText(waitMs) : "" };
}

export const STATUS_TEXT: Record<string, string> = {
  ok: "成功",
  rejected: "拒绝",
  error: "错误",
  cancelled: "已取消",
};
export const statusText = (s: string | null | undefined): string => STATUS_TEXT[s ?? ""] ?? (s || "错误");
export const statusTone = (s: string): "ok" | "warn" | "danger" =>
  s === "ok" ? "ok" : s === "rejected" || s === "cancelled" ? "warn" : "danger";
