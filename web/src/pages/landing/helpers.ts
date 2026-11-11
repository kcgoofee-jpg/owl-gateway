import { V5_PAUSED_HINT } from "./status";
import type { Me, MeMember, ServerQuotaRow } from "./types";

/** /public/me 的响应（或网关注入的 owl-boot.me）长得像不像一个 Me。 */
export const isMe = (v: unknown): v is Me =>
  typeof v === "object" && v !== null && typeof (v as { logged_in?: unknown }).logged_in === "boolean";

/** 重置券到期倒计时 hh:mm:ss，小时可以超过 24（三天就是 72:00:00）；过期了是 00:00:00。 */
export function countdown(expiresAt: number, nowSec = Date.now() / 1000): string {
  const total = Math.max(0, Math.floor(expiresAt - nowSec));
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${pad(Math.floor(total / 3600))}:${pad(Math.floor(total / 60) % 60)}:${pad(total % 60)}`;
}

/** 上游账号的免费 V5 都用完了：没有重置时间，只能等逐步恢复 */
const V5_EXHAUSTED_HINT = "全站 V5 免费额度暂时用完，正在逐步恢复，可以先用 V4.5";

const LABELS = { legacy: "V4.5", v5: "V5", anlas: "Anlas" } as const;

export type QuotaRow = {
  id: "legacy" | "v5" | "anlas";
  label: string;
  used: number;
  /** 上限（0 = 这一档停用） */
  cap: number;
  hint?: string;
  /** 实际还能用的量。服务端已经取过个人剩余和全站剩余的较小值。 */
  left?: number;
  /** 停用 / 用完：圆环显示为暂停 */
  paused?: boolean;
};

/** 把服务端的原因码换成首页上的那句话。数字和是否暂停都用 rows 里的，这里不算。 */
function hint(row: ServerQuotaRow, hour: number): string | undefined {
  switch (row.reason) {
    case "disabled":
      if (row.id === "legacy" || (row.cap ?? 0) === 0) {
        return row.id === "v5" ? "V5 已停用（个人额度为 0）" : "V4.5 已停用（个人额度为 0）";
      }
      return row.id === "anlas" ? "全站 Anlas 已停用" : "全站 V5 已停用";
    case "member_off":
      return "此成员未开放 V5";
    case "site_paused":
      return V5_PAUSED_HINT;
    case "upstream_exhausted":
      return V5_EXHAUSTED_HINT;
    case "site_empty":
      return row.id === "anlas"
        ? `全站今天的 Anlas 额度用完了，${hour} 点重置后恢复`
        : `全站今天的 V5 额度用完了，${hour} 点重置后恢复`;
    default:
      return undefined;
  }
}

/** 成员今日额度行。没有 `today.rows` 就不显示（不再用原始计数自己推断）。 */
export function quotaRows(me: MeMember): QuotaRow[] {
  const hour = resetHour(me);
  return (me.today?.rows ?? []).flatMap((row) => {
    if (row.id !== "legacy" && row.id !== "v5" && row.id !== "anlas") return [];
    const text = hint(row, hour);
    return [
      {
        id: row.id,
        label: LABELS[row.id],
        used: row.used ?? 0,
        cap: row.cap ?? 0,
        ...(row.left === undefined ? {} : { left: row.left }),
        ...(row.paused === undefined ? {} : { paused: row.paused }),
        ...(text === undefined ? {} : { hint: text }),
      },
    ];
  });
}

/** 每日重置的整点：接口给什么用什么（成员才有），拿不到按站长规定的 7 点。 */
export const resetHour = (me: MeMember | null | undefined): number => me?.today?.reset_hour ?? 7;

export const displayName = (me: MeMember): string =>
  me.discord?.display_name || me.discord?.username || "成员";
