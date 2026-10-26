import { V5_PAUSED_HINT } from "./status";
import type { Me, MeMember } from "./types";

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

export type QuotaRow = {
  id: "legacy" | "v5" | "anlas";
  label: string;
  used: number;
  /** 上限（0 = 停用，10-27 规范化后存量里不再有 0） */
  cap: number;
  hint?: string;
  /** 实际还能用的张数（比 cap - used 更少时给出，例如被全站剩余量卡住） */
  left?: number;
  /** 停用 / 用完：圆环显示为暂停 */
  paused?: boolean;
};

/**
 * 成员今日额度三行（10-27 规范化：上限是数字，0 = 停用）：
 * - V4.5：用 today.legacy（免费图张数），没有就退回 images − v5；上限 daily_images，0 = 停用
 * - V5：上限 daily_v5，0 = 停用；全站也有 V5 日池时，能用的是「个人剩余」和「全站剩余」的较小值
 * - Anlas：只有配了 daily_anlas 才显示
 */
export function quotaRows(me: MeMember): QuotaRow[] {
  const t = me.today ?? {};
  const v5 = t.v5 ?? 0;
  const legacy = t.legacy != null ? t.legacy : Math.max(0, (t.images ?? 0) - v5);
  const capImages = (t.daily_images ?? 0) > 0 ? (t.daily_images ?? 0) : 0;
  const rows: QuotaRow[] = [
    capImages === 0
      ? {
          id: "legacy",
          label: "V4.5",
          used: legacy,
          cap: 0,
          paused: true,
          hint: "V4.5 已停用（个人额度为 0）",
        }
      : { id: "legacy", label: "V4.5", used: legacy, cap: capImages },
  ];
  const capV5 = (t.daily_v5 ?? 0) > 0 ? (t.daily_v5 ?? 0) : 0;
  if (capV5 > 0 || me.image_scope === "all") {
    const row: QuotaRow = { id: "v5", label: "V5", used: v5, cap: capV5 };
    if (capV5 === 0) {
      Object.assign(row, { paused: true, left: 0, hint: "V5 已停用（个人额度为 0）" });
    }
    // 全站也有 V5 日上限时，能用的是「个人剩余」和「全站剩余」的较小值；null = 这把 Key 不计入全站
    else {
      const site = t.site_v5_left;
      if (t.v5_access_allowed === false) {
        Object.assign(row, { paused: true, left: 0, hint: "当前访问策略未授权 V5" });
      } else if (t.v5_paused) {
        Object.assign(row, { paused: true, left: 0, hint: V5_PAUSED_HINT });
      } else if (t.upstream_v5_free === false) {
        Object.assign(row, { paused: true, left: 0, hint: V5_EXHAUSTED_HINT });
      } else if (site != null && site <= 0) {
        Object.assign(row, {
          paused: true,
          left: 0,
          hint: `全站今天的 V5 额度用完了，${resetHour(me)} 点重置后恢复`,
        });
      } else if (site != null) {
        row.left = Math.min(Math.max(0, capV5 - v5), site);
      }
    }
    rows.push(row);
  }
  if ((t.daily_anlas ?? 0) > 0) {
    rows.push({ id: "anlas", label: "Anlas", used: t.anlas ?? 0, cap: t.daily_anlas ?? 0 });
  }
  return rows;
}

/** 每日重置的整点：接口给什么用什么（成员才有），拿不到按站长规定的 7 点。 */
export const resetHour = (me: MeMember | null | undefined): number => me?.today?.reset_hour ?? 7;

export const displayName = (me: MeMember): string =>
  me.discord?.display_name || me.discord?.username || "成员";
