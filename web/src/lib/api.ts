/**
 * 后端调用。错误统一成 ApiError：网关的错误体是 {statusCode, message, error:{message, status, request_id}}
 * （与 NovelAI 官方格式一致，客户端直接显示 message）。
 */
export class ApiError extends Error {
  retryAfter?: number;
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
    const error = new ApiError(res.status, msg, b?.error?.request_id);
    const retry = Number(res.headers.get("Retry-After"));
    if (Number.isFinite(retry) && retry > 0) error.retryAfter = Math.ceil(retry);
    throw error;
  }
  return body;
}

export async function getJSON<T>(url: string, init?: RequestInit): Promise<T> {
  // 有界超时 + 一次重试：后台曾出现请求无限挂起、骨架屏一直转（见 docs/backend-audit-2026-11-01.md）。
  // GET 幂等可安全重试；写操作（sendJSON）只给超时，不自动重发。
  let lastError: unknown;
  for (let attempt = 0; attempt < 2; attempt++) {
    try {
      return await request<T>("GET", url, undefined, init);
    } catch (e) {
      lastError = e;
      // 4xx/5xx 是服务端明确回答，重试没有意义；只重试网络层失败（超时 / 断开）。
      if (e instanceof ApiError && e.status !== 0) throw e;
      if (attempt === 0) await new Promise((r) => setTimeout(r, 600));
    }
  }
  throw lastError;
}

const REQUEST_TIMEOUT_MS = 20_000;

async function request<T>(method: string, url: string, body?: unknown, init?: RequestInit): Promise<T> {
  const ctrl = new AbortController();
  const timer = setTimeout(() => ctrl.abort(), REQUEST_TIMEOUT_MS);
  try {
    const res = await fetch(url, {
      credentials: "same-origin",
      ...init,
      method,
      headers: {
        Accept: "application/json",
        ...(body === undefined ? {} : { "Content-Type": "application/json" }),
        ...((init?.headers as Record<string, string> | undefined) ?? {}),
      },
      body: body === undefined ? null : JSON.stringify(body),
      signal: ctrl.signal,
    });
    return (await parse(res)) as T;
  } catch (e) {
    if (e instanceof ApiError) throw e;
    // abort / 网络错误：给用户一句可读的，而不是裸的 "The operation was aborted"
    throw new ApiError(0, `请求超时或网络中断（${REQUEST_TIMEOUT_MS / 1000} 秒），稍后会自动重试。`);
  } finally {
    clearTimeout(timer);
  }
}

export async function sendJSON<T>(
  method: string,
  url: string,
  body?: unknown,
  headers?: Record<string, string>,
): Promise<T> {
  return request<T>(method, url, body, headers ? { headers } : undefined);
}
