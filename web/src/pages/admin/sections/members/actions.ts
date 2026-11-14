/**
 * 成员相关的写操作。统一返回 {ok, data} / {ok:false, error}：
 * 原生 <dialog> 盖在 toast 上面，所以出错要把原因交给对话框自己显示，成功再由调用方 toast。
 * 成功后触发一次全局刷新（tick）。
 */
import { adminWrite, type WriteResult } from "../../data";
import type { Coupon } from "./types";

export type Res<T = unknown> = WriteResult<T>;

export async function call<T = unknown>(
  method: "POST" | "PUT" | "PATCH" | "DELETE",
  url: string,
  body?: unknown,
): Promise<Res<T>> {
  return adminWrite<T>(method, url, body, { notify: "return" });
}

const A = "/admin/api";

export const disableKey = (id: number) => call("PATCH", `${A}/keys/${id}`, { enabled: false });

export const removeMember = (id: number, ban: boolean) =>
  call("DELETE", `${A}/keys/${id}?ban=${ban ? "true" : "false"}`);

export const regenerateKey = (id: number) => call<{ token: string }>("POST", `${A}/keys/${id}/regenerate`);

/** V4.5 一定重置；V5 / Anlas 要勾选（站长 10-18：默认只重置 V4.5） */
export type ResetScope = { v5: boolean; anlas: boolean };
const scopeList = (s: ResetScope) => [...(s.v5 ? ["v5"] : []), ...(s.anlas ? ["anlas"] : [])];

export const resetDailyQuota = (id: number, scope: ResetScope) =>
  call("POST", `${A}/keys/${id}/reset-daily-image-quota`, { scope: scopeList(scope) });

export const addTag = (id: number, tag: string, note: string) =>
  call("PUT", `${A}/keys/${id}/tags`, { tag, note });
export const removeTag = (id: number, tag: string) =>
  call("DELETE", `${A}/keys/${id}/tags/${encodeURIComponent(tag)}`);
/** 站长手动私信（奶妹发，小批量限频：每小时 10 条、每天 30 条） */
export const dmMember = (id: number, text: string) => call("POST", `${A}/keys/${id}/dm`, { text });
export const clearShare = (id: number) => call("POST", `${A}/keys/${id}/share-clear`);

export const giveCoupon = (id: number, days: number, note: string, scope: ResetScope) =>
  call<{ coupon: Coupon }>("POST", `${A}/keys/${id}/coupons`, {
    kind: "reset",
    days,
    note,
    scope: scopeList(scope),
  });

export const resetAllKeys = () =>
  call<{ message?: string }>("POST", `${A}/ops/reset-all-keys`, { confirm: "确认重置" });

export const addBan = (discordId: string, reason: string) =>
  call("POST", `${A}/bans`, { discord_id: discordId, reason });

export const removeBan = (discordId: string) => call("DELETE", `${A}/bans/${encodeURIComponent(discordId)}`);
