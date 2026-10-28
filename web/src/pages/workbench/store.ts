/**
 * 生图工作台的数据层：只要「我是谁 / 我的额度」。me 用网关注入的 owl-boot 起底，
 * 每次出图成功后 reloadMe 一次（额度行重新充满），另加 60 秒兜底与切回标签页时刷新。
 * 不订阅实时看板：工作台不展示全站排队细节，保持页面轻。
 */
import { signal } from "@preact/signals";
import { getJSON } from "../../lib/api";
import { bootData } from "../../lib/boot";
import { isMe } from "../landing/helpers";
import type { Me } from "../landing/types";

type Boot = { me?: unknown };

const boot = bootData<Boot>();
/** undefined = 还在读（显示骨架屏，不要先闪一下游客内容） */
export const me = signal<Me | undefined>(isMe(boot?.me) ? boot.me : undefined);

export async function reloadMe(): Promise<void> {
  try {
    me.value = await getJSON<Me>("/public/me", { cache: "no-store" });
  } catch {
    // 偶发网络错误不把已登录的人踢成游客；首屏读不到才当游客处理
    if (me.value === undefined) me.value = { logged_in: false };
  }
}

let started = false;
export function start(): void {
  if (started) return;
  started = true;
  if (me.value === undefined) void reloadMe();
  window.setInterval(() => {
    if (!document.hidden) void reloadMe();
  }, 60_000);
  document.addEventListener("visibilitychange", () => {
    if (!document.hidden) void reloadMe();
  });
}
