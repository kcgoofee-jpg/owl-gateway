/**
 * 后端调用。错误统一成 ApiError：网关的错误体是 {statusCode, message, error:{message, status, request_id}}
 * （与 NovelAI 官方格式一致，客户端直接显示 message）。
 */
export class ApiError extends Error {
  constructor(
    public status: number,
    message: string,
    public requestId?: string,
  ) {
    super(message);
  }
}

const NOTICE_PREFIX = "猫头鹰公益站提醒：";

async function parse(res: Response): Promise<unknown> {
  const text = await res.text();
  let body: unknown = null;
  try {
    body = text ? JSON.parse(text) : null;
  } catch {
    body = text;
  }
  if (!res.ok) {
    const b = body as {
      message?: string;
      detail?: string;
      error?: { message?: string; request_id?: string };
    } | null;
    let msg = b?.error?.message ?? b?.message ?? b?.detail ?? `请求失败（HTTP ${res.status}）`;
    if (msg.startsWith(NOTICE_PREFIX)) msg = msg.slice(NOTICE_PREFIX.length);
    throw new ApiError(res.status, msg, b?.error?.request_id);
  }
  return body;
}

export async function getJSON<T>(url: string, init?: RequestInit): Promise<T> {
  const res = await fetch(url, {
    credentials: "same-origin",
    headers: { Accept: "application/json" },
    ...init,
  });
  return (await parse(res)) as T;
}

export async function sendJSON<T>(
  method: string,
  url: string,
  body?: unknown,
  headers?: Record<string, string>,
): Promise<T> {
  const res = await fetch(url, {
    method,
    credentials: "same-origin",
    headers: {
      Accept: "application/json",
      ...(body === undefined ? {} : { "Content-Type": "application/json" }),
      ...headers,
    },
    body: body === undefined ? null : JSON.stringify(body),
  });
  return (await parse(res)) as T;
}

/**
 * 浏览器网络失败的报错文案（断网、切后台、页面跳转时被取消的请求）：不是 Bug，不上报。
 * 清单与服务端 `gate_core::errors::is_network_failure` 一致（服务端同样会丢弃，这里先不发）。
 */
const NETWORK_NEEDLES = [
  "failed to fetch",
  "networkerror",
  "load failed",
  "network error",
  "the network connection was lost",
  "aborterror",
  "the operation was aborted",
  "cancelled",
  "canceled",
  "已取消",
];
export function isNetworkFailure(msg: string): boolean {
  const m = msg.toLowerCase();
  return NETWORK_NEEDLES.some((n) => m.includes(n));
}

/**
 * 前端报错上报（与旧版同一接口与字段：page / msg / src / line / stack；只报本站脚本的错误）。
 * 脚本错误（ErrorEvent）另带 `filename`（出错脚本的地址）：服务端据此丢弃不在 `/_owl/` 下的报错
 * （旧版缓存页、浏览器插件注入的脚本）。网络失败不报。
 */
export function installErrorReporter(page: "landing" | "admin" | "survey"): void {
  let sent = 0;
  const report = (msg: string, src?: string, line?: number, stack?: string, filename?: string) => {
    if (!msg || msg === "Script error." || isNetworkFailure(msg) || sent++ > 5) return;
    const body = JSON.stringify({
      page,
      msg: msg.slice(0, 300),
      src: (src ?? location.pathname).slice(0, 200),
      line: line ?? 0,
      stack: (stack ?? "").slice(0, 1500),
      ...(filename === undefined ? {} : { filename: filename.slice(0, 300) }),
    });
    navigator.sendBeacon?.("/public/client-error", new Blob([body], { type: "application/json" }));
  };
  addEventListener("error", (e) => report(e.message, e.filename, e.lineno, e.error?.stack, e.filename ?? ""));
  addEventListener("unhandledrejection", (e) =>
    report(String(e.reason?.message ?? e.reason), undefined, undefined, e.reason?.stack),
  );
}
