/** 日志相关的显示格式化（纯函数）：功能名、耗时。模型短名和交付徽标在 lib/request-label。 */
import { secondsText } from "../../../../lib/format";
import { type Feature, RETIRED } from "../features";
import type { UsageLog } from "./types";

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

/** 行上的主状态文案：拒绝看原因，失败看上游分类，不再只写「错误 / 上游 503」。 */
export function statusHeadline(
  l: Pick<UsageLog, "status" | "reason_label" | "issue_label" | "up_status">,
): string {
  if (l.status === "rejected") return l.reason_label || "拒绝";
  if (l.status === "error") {
    if (l.issue_label) return l.issue_label;
    return l.up_status ? `上游 ${l.up_status}` : "错误";
  }
  return statusText(l.status);
}

/** 展开详情里对上游失败的一句话，不把请求形状当成原因。 */
export const ISSUE_EXPLAIN = {
  up_503: "出图通道繁忙或维护。括号里是当时的请求形状，不是失败原因。",
  up_5xx: "出图通道暂时故障，不是成员把参数写错。",
  up_422: "上游没接受这组参数。若点了未知字段，多半是客户端多带了官网字段。",
  up_429: "上游正在限流，网关会进入冷却。",
  up_401: "本站上游账号登录失效，需要站长处理。",
  up_402: "上游账号额度不足。",
  up_403: "这次请求被上游拒绝。",
  up_404: "上游暂不支持该功能或模型。",
  up_413: "请求内容过大，缩小图片或减少参考图。",
  up_other: "上游返回了未归类的错误。",
} as const;

export function issueExplain(code: string | undefined): string {
  if (code && code in ISSUE_EXPLAIN) return ISSUE_EXPLAIN[code as keyof typeof ISSUE_EXPLAIN];
  return ISSUE_EXPLAIN.up_other;
}
