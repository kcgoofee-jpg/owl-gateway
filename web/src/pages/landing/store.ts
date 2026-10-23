/**
 * 首页的数据层：一个页面一份，模块级 signal。
 * - me：/public/me（首屏用网关注入的 owl-boot，没有就取一次；之后每分钟、切回标签页时刷新）
 * - live：/public/live（SSE 优先，旧网关没有 SSE 就轮询；登录成员的响应里带 me = 我的排队）
 * - status：/public/status（站点信息，30 秒一次）
 * 组件只读自己要的那一小块（读 .value 的组件才会重绘），所以每秒一次的 live 不会带着整页重绘。
 */
import { computed, signal } from "@preact/signals";
import { getJSON } from "../../lib/api";
import { bootData } from "../../lib/boot";
import { subscribe } from "../../lib/live";
import { isMe } from "./helpers";
import type { Live, Me, Meta } from "./types";

type Boot = { me?: unknown; live?: unknown; status?: unknown };

const isObj = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null;

/** live 里带的站点信息比 /public/status 更新（≤30 秒 vs 每 30 秒拉一次）；live 里没有的字段（旧/新网关差异）用 status 的。 */
const META_KEYS = ["discord_invite", "registration", "limits"] as const satisfies ReadonlyArray<keyof Meta>;

export function mergeMeta(status: Meta | null, live: Live | null): Meta {
  const out: Record<string, unknown> = {};
  for (const k of META_KEYS) {
    const v = live?.[k] ?? status?.[k];
    if (v !== undefined && v !== null) out[k] = v;
  }
  return out as Meta;
}

/** 内容没变就返回同一个对象：下游组件不会因为 live 每秒换了个新对象而重绘。 */
function stable<T>(fn: () => T) {
  let prev: T | undefined;
  let prevJson = "";
  return computed<T>(() => {
    const v = fn();
    const j = JSON.stringify(v) ?? "";
    if (j === prevJson) return prev as T;
    prev = v;
    prevJson = j;
    return v;
  });
}

export function createStore() {
  const boot = bootData<Boot>();
  /** undefined = 还在读（显示骨架屏，不要先闪一下游客内容） */
  const me = signal<Me | undefined>(isMe(boot?.me) ? boot.me : undefined);
  const status = signal<Meta | null>(isObj(boot?.status) ? (boot.status as Meta) : null);
  const liveSub = subscribe<Live>(
    "/public/live/stream",
    "/public/live",
    isObj(boot?.live) ? (boot.live as Live) : null,
  );
  const live = liveSub.data;
  const meta = stable<Meta>(() => mergeMeta(status.value, live.value));
  /** 排队估算与规则说明用的那几个字段：只在它们变化时才通知 */
  const facts = stable(() => ({
    cycle: live.value?.cycle ?? 0,
  }));

  const loadMe = async () => {
    try {
      me.value = await getJSON<Me>("/public/me", { cache: "no-store" });
    } catch {
      // 偶发网络错误不把已登录的人踢成游客；首屏读不到才当游客处理
      if (me.value === undefined) me.value = { logged_in: false };
    }
  };
  const loadStatus = async () => {
    try {
      status.value = await getJSON<Meta>("/public/status", { cache: "no-store" });
    } catch {
      /* 保留旧值；live 的 meta 也能补 */
    }
  };

  /** 在页面挂载时调用，返回清理函数。 */
  const start = () => {
    if (me.value === undefined) void loadMe();
    void loadStatus();
    const timer = setInterval(() => {
      if (document.hidden) return;
      void loadMe();
      void loadStatus();
    }, 60_000);
    const onVisible = () => {
      if (!document.hidden) {
        void loadMe();
        void loadStatus();
      }
    };
    document.addEventListener("visibilitychange", onVisible);
    return () => {
      clearInterval(timer);
      document.removeEventListener("visibilitychange", onVisible);
    };
  };

  return { me, live, connection: liveSub.connected, meta, facts, reloadMe: loadMe, start };
}

export type Store = ReturnType<typeof createStore>;
