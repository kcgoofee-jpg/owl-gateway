/**
 * 生图工作台的网络层：只打网关的 NAI 兼容出图接口（Bearer = 成员自己的 Key）。
 * 错误体与网关一致：{statusCode, message, error:{message, request_id, code, retryable, execution_unknown}}；
 * 429 会带 Retry-After 头，批量队列靠它做限速重试。
 */
import { type GenAttrs, parseSuggestions } from "../../lib/nai-config";
import { extractImages, type ImageBytes, imageExt, toDataUrl } from "../../lib/nai-image";

/** 一张成图：data URL（CSP 允许 img data:，不允许 blob:）、扩展名、冻结的参数、批量时的来源标签。 */
export type Shot = {
  url: string;
  ext: string;
  b64: string;
  attrs: GenAttrs;
  label?: string | undefined;
};

export function bytesToBase64(bytes: Uint8Array): string {
  let bin = "";
  const step = 0x8000;
  for (let i = 0; i < bytes.length; i += step) {
    const slice = bytes.subarray(i, Math.min(i + step, bytes.length));
    let part = "";
    for (const byte of slice) part += String.fromCharCode(byte);
    bin += part;
  }
  return btoa(bin);
}

export function makeShot(img: ImageBytes, attrs: GenAttrs, label?: string): Shot {
  return { url: toDataUrl(img), ext: imageExt(img.mime), b64: bytesToBase64(img.bytes), attrs, label };
}

/** 网关给成员看的提示都带这个前缀，界面上不重复念。 */
const NOTICE_PREFIX = "猫头鹰公益站提醒：";

export class WorkbenchError extends Error {
  status: number;
  /** 限速且明确没开始画：批量队列可以等 Retry-After 再试。 */
  retryable: boolean;
  /** 网关不确定这次是否已经出图：不能自动重试，只能停下来让人核对。 */
  executionUnknown: boolean;
  retryAfter: number | null;

  constructor(init: {
    status: number;
    retryable?: boolean;
    executionUnknown?: boolean;
    retryAfter?: number | null;
    message: string;
  }) {
    super(init.message);
    this.name = "WorkbenchError";
    this.status = init.status;
    this.retryable = init.retryable === true;
    this.executionUnknown = init.executionUnknown === true;
    this.retryAfter = init.retryAfter ?? null;
  }
}

const clean = (m: string) => m.replace(NOTICE_PREFIX, "");

async function readError(res: Response, fallback: string): Promise<never> {
  const retryAfter = Number(res.headers.get("retry-after"));
  let err: { message?: string; retryable?: boolean; execution_unknown?: boolean } = {};
  let top: { message?: string } = {};
  try {
    const body = (await res.json()) as { message?: string; error?: typeof err };
    top = body;
    err = body.error ?? {};
  } catch {
    /* 错误体不是 JSON（反代页等）：用兜底文案 */
  }
  throw new WorkbenchError({
    status: res.status,
    retryable: err.retryable === true,
    executionUnknown: err.execution_unknown === true,
    retryAfter: Number.isFinite(retryAfter) && retryAfter > 0 ? retryAfter : null,
    message: clean(err.message ?? top.message ?? fallback),
  });
}

async function postJson(path: string, body: unknown, key: string, signal: AbortSignal): Promise<Response> {
  try {
    return await fetch(path, {
      method: "POST",
      headers: { authorization: `Bearer ${key}`, "content-type": "application/json" },
      body: JSON.stringify(body),
      signal,
    });
  } catch (e) {
    if (e instanceof DOMException && e.name === "AbortError") throw e;
    throw new WorkbenchError({
      status: 0,
      executionUnknown: true,
      message: "网络中断了。如果网关已经开始画，这次仍可能计入今天的额度——稍后在「我的请求」里核对。",
    });
  }
}

/** 发一次出图请求；成功返回解开的图片（PNG 直给 / 多张 ZIP 解包）。 */
export async function postImages(body: unknown, key: string, signal: AbortSignal): Promise<ImageBytes[]> {
  const res = await postJson("/ai/generate-image", body, key, signal);
  if (!res.ok) await readError(res, `出图没有成功（${res.status}）。`);
  return extractImages(new Uint8Array(await res.arrayBuffer()));
}

/** V4 / V4.5 Vibe：网关返回二进制编码，调用方再 base64 放进出图请求。 */
export async function encodeVibe(
  body: { image: string; model: string; informationExtracted: number },
  key: string,
  signal: AbortSignal,
): Promise<string> {
  const res = await postJson("/ai/encode-vibe", body, key, signal);
  if (!res.ok) await readError(res, `Vibe 编码没有成功（${res.status}）。`);
  return bytesToBase64(new Uint8Array(await res.arrayBuffer()));
}

/** 放大或导演工具。返回和出图一样的图片字节。 */
export async function postTool(
  path: "/ai/upscale" | "/ai/augment-image",
  body: unknown,
  key: string,
  signal: AbortSignal,
): Promise<ImageBytes[]> {
  const res = await postJson(path, body, key, signal);
  if (!res.ok) await readError(res, `图片工具没有成功（${res.status}）。`);
  return extractImages(new Uint8Array(await res.arrayBuffer()));
}

/** 标签补全（policy.tags 开着才显示入口）。 */
export async function fetchSuggestions(
  prompt: string,
  model: string,
  key: string,
  signal: AbortSignal,
): Promise<string[]> {
  const qs = new URLSearchParams({ prompt, model });
  const res = await fetch(`/ai/generate-image/suggest-tags?${qs}`, {
    headers: { authorization: `Bearer ${key}` },
    signal,
  });
  if (!res.ok) {
    const body = (await res.json().catch(() => ({}))) as { error?: { message?: string } };
    throw new Error(clean(body.error?.message ?? "标签补全没有成功，请稍后再试。"));
  }
  return parseSuggestions(await res.json());
}
