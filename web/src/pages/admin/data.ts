import { signal } from "@preact/signals";
import { useEffect, useState } from "preact/hooks";
import { ApiError, getJSON, sendJSON } from "../../lib/api";
import { toast } from "../../ui";

/** 后台写入完成后，冻结的列表也要接收新的结果。 */
export const edits = signal(0);
/* ---------------- 全局刷新节拍（手动） ---------------- */
/** 每次 +1 都会让所有 useAdmin 重新取数。站长 2026-11-14 改手动：只由刷新按钮与写操作驱动，不再 SSE / 定时轮询。 */
export const tick = signal(0);
/** 用户正在编辑表单 / 打开对话框时禁用手动刷新，避免把输入冲掉。 */
export const busy = signal(0);

/** 手动刷新入口（后台顶栏按钮）。 */
export function refresh(): void {
  tick.value++;
}

/** 跨分区缓存：切 tab 先秒显上次结果，只有刷新 / 写操作（tick 变化）后才重取。 */
const cache = new Map<string, unknown>();
const fetchedTick = new Map<string, number>();

/** 慢请求跨自动刷新复用；写操作之后必须重新请求。只保留最近 32 个地址的结果。 */
const inflight = new Map<string, { t: number; edits: number; pending: boolean; p: Promise<unknown> }>();
export function getShared<T>(url: string, t = tick.value): Promise<T> {
  const hit = inflight.get(url);
  if (hit && hit.edits === edits.value && (hit.t === t || hit.pending)) {
    hit.t = t;
    return hit.p as Promise<T>;
  }
  const p = getJSON<T>(url);
  const entry = { t, edits: edits.value, pending: true, p };
  inflight.delete(url);
  if (inflight.size >= 32) inflight.delete(inflight.keys().next().value as string);
  inflight.set(url, entry);
  p.then(
    () => {
      entry.pending = false;
    },
    () => {
      if (inflight.get(url) === entry) inflight.delete(url);
    },
  );
  return p;
}

/** 取后台数据：url 为 null 时不取。返回 {data, error, loading}。
 *  切页 / 重挂载先秒显跨分区缓存；只有手动刷新或写操作（tick 变化）后才重取。 */
export function useAdmin<T>(url: string | null): {
  data: T | null;
  error: string | null;
  loading: boolean;
} {
  const [state, setState] = useState<{ url: string; data: T | null; error: string | null } | null>(() =>
    url && cache.has(url) ? { url, data: cache.get(url) as T, error: null } : null,
  );
  const t = tick.value;
  useEffect(() => {
    if (!url) return;
    // 缓存已是当前节拍的结果：秒显、不发请求（切 tab 不重取）。
    if (cache.has(url) && fetchedTick.get(url) === t) {
      setState((cur) =>
        cur?.url === url && cur.error === null ? cur : { url, data: cache.get(url) as T, error: null },
      );
      return;
    }
    let alive = true;
    getShared<T>(url, t)
      .then((data) => {
        cache.set(url, data);
        fetchedTick.set(url, t);
        // 与 getShared 同样的上限：日志筛选/翻页会产生大量地址，不能无限留着。
        if (cache.size > 32) {
          const old = cache.keys().next().value as string;
          cache.delete(old);
          fetchedTick.delete(old);
        }
        if (alive)
          setState((current) =>
            current?.url === url && current.data === data && current.error === null
              ? current
              : { url, data, error: null },
          );
      })
      .catch((e: unknown) => {
        if (!alive) return;
        if (e instanceof ApiError && e.status === 401) endSession();
        setState((s) => ({
          url,
          data: s?.url === url ? s.data : ((cache.get(url) as T | undefined) ?? null),
          error: e instanceof Error ? e.message : String(e),
        }));
      });
    return () => {
      alive = false;
    };
  }, [url, t]);
  if (!state || state.url !== url) return { data: null, error: null, loading: !!url };
  return { data: state.data, error: state.error, loading: false };
}

export type WriteResult<T> = { ok: true; data: T } | { ok: false; error: string };

/**
 * 后台写操作。成功后触发一次全局刷新。
 * notify 为 toast 时成功和失败都用 toast；为 return 时把错误交回调用方（对话框盖在 toast 上面）。
 * 401 都回到登录页。
 */
export async function adminWrite<T = unknown>(
  method: "POST" | "PUT" | "PATCH" | "DELETE",
  url: string,
  body?: unknown,
  options?: { okText?: string; notify?: "toast" | "return"; headers?: Record<string, string> },
): Promise<WriteResult<T>> {
  const notify = options?.notify ?? "toast";
  const okText = options?.okText ?? (notify === "toast" ? "已保存" : "");
  try {
    const data = await sendJSON<T>(method, url, body, options?.headers);
    if (notify === "toast" && okText) toast(okText);
    edits.value++;
    tick.value++;
    return { ok: true, data };
  } catch (e) {
    if (e instanceof ApiError && e.status === 401) endSession();
    const error = e instanceof Error ? e.message : String(e);
    if (notify === "toast") toast(error, { error: true });
    return { ok: false, error };
  }
}

/** 成功 toast，失败 toast 错误；返回结果或 null。 */
export async function act<T = unknown>(
  method: "POST" | "PUT" | "PATCH" | "DELETE",
  url: string,
  body?: unknown,
  okText = "已保存",
): Promise<T | null> {
  const result = await adminWrite<T>(method, url, body, { okText, notify: "toast" });
  return result.ok ? result.data : null;
}

/* ---------------- 登录 ---------------- */
export const authed = signal<boolean | null>(null);

export function endSession() {
  inflight.clear();
  cache.clear();
  fetchedTick.clear();
  authed.value = false;
}
