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
  /** 0 = 不限 */
  cap: number;
  hint?: string;
  /** 实际还能用的张数（比 cap - used 更少时给出，例如被全站剩余量卡住） */
  left?: number;
  /** 全站今天的额度用完了：圆环显示为暂停 */
  paused?: boolean;
};

/**
 * 成员今日额度三行（字段含义沿用旧版首页）：
 * - V4.5：用 today.legacy（免费图张数），没有就退回 images − v5；上限 daily_images（0 = 不限）
 * - V5：上限 daily_v5；没配上限且这把 Key 的模型范围不是 all 时整行隐藏；0 + all = 个人不限（仍受全站日上限约束）
 * - Anlas：只有配了 daily_anlas 才显示
 */
export function quotaRows(me: MeMember): QuotaRow[] {
  const t = me.today ?? {};
  const v5 = t.v5 ?? 0;
  const legacy = t.legacy != null ? t.legacy : Math.max(0, (t.images ?? 0) - v5);
  const rows: QuotaRow[] = [
    { id: "legacy", label: "V4.5", used: legacy, cap: (t.daily_images ?? 0) > 0 ? (t.daily_images ?? 0) : 0 },
  ];
  const capV5 = (t.daily_v5 ?? 0) > 0 ? (t.daily_v5 ?? 0) : 0;
  if (capV5 > 0 || me.image_scope === "all") {
    const row: QuotaRow = {
      id: "v5",
      label: "V5",
      used: v5,
      cap: capV5,
      ...(capV5 === 0 ? { hint: "个人不限额度，仍受全站日上限约束" } : {}),
    };
    // 全站也有 V5 日上限时，能用的是「个人剩余」和「全站剩余」的较小值；全站不设限（null）只看个人
    const site = t.site_v5_left;
    const siteCap = t.site_v5_cap ?? 0;
    if (t.v5_paused) {
      Object.assign(row, { paused: true, left: 0, hint: V5_PAUSED_HINT });
    } else if (t.upstream_v5_free === false) {
      Object.assign(row, { paused: true, left: 0, hint: V5_EXHAUSTED_HINT });
    } else if (site != null) {
      if (site <= 0) {
        Object.assign(row, {
          paused: true,
          left: 0,
          hint: `全站今天的 V5 额度用完了，${resetHour(me)} 点重置后恢复`,
        });
      } else if (capV5 > 0) {
        row.left = Math.min(Math.max(0, capV5 - v5), site);
      } else if (siteCap > 0) {
        // 个人不限：圆环按全站的量画
        Object.assign(row, { cap: siteCap, used: siteCap - site, left: site });
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
