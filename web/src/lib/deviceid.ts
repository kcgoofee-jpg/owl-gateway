/**
 * 浏览器设备指纹：收集几样跨重启、换 profile 都稳定的信号（Canvas、WebGL、屏幕、时区、硬件），发给服务端，
 * 由服务端算出短哈希 id 回给前端；网页登录时带上（/login?dev=…），用来发现「同一台设备登录了多个 Discord 账号」= 疑似小号。
 *
 * 补充项（语音 / 字体 / 色域 / HDR / 像素比 / 存储档位 / UA-CH）和电量快照另外上报（`extra` / `battery`），不影响指纹 id。
 *
 * 只用于站长的防滥用核查；算不出来就返回空串，登录照常。专业的反检测浏览器能伪造这些值，对普通用户够用。
 */

import { signal } from "@preact/signals";
import { useEffect } from "preact/hooks";

async function sha256hex(s: string): Promise<string> {
  const buf = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(s));
  return Array.from(new Uint8Array(buf))
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("");
}

/** Canvas 渲染指纹：同样的绘制在不同显卡 / 驱动 / 系统上像素有细微差别。 */
function canvasFp(): string {
  try {
    const c = document.createElement("canvas");
    c.width = 240;
    c.height = 60;
    const ctx = c.getContext("2d");
    if (!ctx) return "";
    ctx.textBaseline = "top";
    ctx.font = "14px 'Arial'";
    ctx.fillStyle = "#f60";
    ctx.fillRect(10, 10, 100, 30);
    ctx.fillStyle = "#069";
    ctx.fillText("猫头鹰 owl \u{1F989} 0123", 12, 14);
    ctx.fillStyle = "rgba(102,204,0,0.65)";
    ctx.fillText("owl fingerprint", 16, 28);
    return c.toDataURL();
  } catch {
    return "";
  }
}

/** WebGL 指纹：显卡厂商 / 型号 + 一组能力参数，稳定度高。 */
function webglFp(): string {
  try {
    const c = document.createElement("canvas");
    const gl = (c.getContext("webgl") || c.getContext("experimental-webgl")) as WebGLRenderingContext | null;
    if (!gl) return "";
    const dbg = gl.getExtension("WEBGL_debug_renderer_info");
    const vendor = dbg ? gl.getParameter(dbg.UNMASKED_VENDOR_WEBGL) : "";
    const renderer = dbg ? gl.getParameter(dbg.UNMASKED_RENDERER_WEBGL) : "";
    const params = [
      gl.getParameter(gl.MAX_TEXTURE_SIZE),
      gl.getParameter(gl.MAX_RENDERBUFFER_SIZE),
      gl.getParameter(gl.MAX_VERTEX_ATTRIBS),
      (gl.getSupportedExtensions() || []).join(","),
    ].join("~");
    return `${vendor}~${renderer}~${params}`;
  } catch {
    return "";
  }
}

/** 可读的设备分项（站长查小号时能看清「哪几项一样」）。 */
type Parts = Record<string, string | number>;

function attrs(): Parts {
  const nav = navigator as Navigator & { deviceMemory?: number };
  const [wglVendor, wglRenderer] = webglFp().split("~");
  return {
    ua: nav.userAgent,
    lang: nav.language,
    langs: (nav.languages || []).join(","),
    screen: `${screen.width}x${screen.height}x${screen.colorDepth}`,
    tz: new Date().getTimezoneOffset(),
    hw: nav.hardwareConcurrency ?? "",
    mem: nav.deviceMemory ?? "",
    platform: nav.platform ?? "",
    touch: nav.maxTouchPoints ?? "",
    gpuVendor: wglVendor || "",
    gpuRenderer: wglRenderer || "",
  };
}

/**
 * 收集设备指纹各项：可读分项（显卡、屏幕、时区、硬件…）加上 Canvas 和 WebGL 全量能力串的哈希
 * （各取 sha256 前 16 位）。指纹 id 由服务端按这些项算（/public/device-fp），前端不自己算。
 */
export async function fingerprint(): Promise<Parts> {
  const parts = attrs();
  parts.canvas = (await sha256hex(canvasFp()).catch(() => "")).slice(0, 16);
  const gl = webglFp();
  parts.webgl = gl ? (await sha256hex(gl).catch(() => "")).slice(0, 16) : "";
  return parts;
}

/* ---------------- 补充项（站长 10-23）：另外上报，不参与指纹 id 的计算，已有的关联不受影响 ---------------- */

/** 系统语音列表（各平台 / 语言包装得不一样）：等 voiceschanged 最多 1 秒。 */
async function voicesFp(): Promise<string> {
  const synth = typeof speechSynthesis === "undefined" ? null : speechSynthesis;
  if (!synth) return "";
  const read = () =>
    synth
      .getVoices()
      .map((v) => `${v.name}|${v.lang}|${v.localService ? 1 : 0}`)
      .sort()
      .join(",");
  let list = read();
  if (!list) {
    list = await new Promise<string>((resolve) => {
      const done = () => resolve(read());
      const t = setTimeout(done, 1000);
      synth.addEventListener?.(
        "voiceschanged",
        () => {
          clearTimeout(t);
          done();
        },
        { once: true },
      );
    });
  }
  return list ? (await sha256hex(list)).slice(0, 16) : "";
}

/** 字体探测要量的字体（常见的中英文字体，装没装因系统和个人习惯而异）。 */
const FONTS = [
  "Arial",
  "Arial Black",
  "Calibri",
  "Cambria",
  "Candara",
  "Comic Sans MS",
  "Consolas",
  "Courier New",
  "Georgia",
  "Helvetica",
  "Helvetica Neue",
  "Impact",
  "Lucida Console",
  "Menlo",
  "Monaco",
  "Palatino",
  "Segoe UI",
  "Tahoma",
  "Times New Roman",
  "Trebuchet MS",
  "Verdana",
  "Roboto",
  "Noto Sans",
  "Ubuntu",
  "SimSun",
  "SimHei",
  "Microsoft YaHei",
  "Microsoft JhengHei",
  "KaiTi",
  "FangSong",
  "DengXian",
  "PingFang SC",
  "Hiragino Sans GB",
  "Heiti SC",
  "STHeiti",
  "Songti SC",
  "Source Han Sans SC",
  "Noto Sans CJK SC",
  "WenQuanYi Micro Hei",
  "MiSans",
  "HarmonyOS Sans SC",
];

/** 字体探测：用每种字体（回退到等宽 / 衬线）画同一串字，宽度和回退字体不同 = 装了这个字体。 */
function fontsFp(): string {
  try {
    const ctx = document.createElement("canvas").getContext("2d");
    if (!ctx) return "";
    const text = "mmmmmmmmmmlli 猫头鹰公益站 WwQq 0123";
    const width = (font: string) => {
      ctx.font = `72px ${font}`;
      return ctx.measureText(text).width;
    };
    const bases = ["monospace", "serif", "sans-serif"];
    const base = bases.map(width);
    return FONTS.map((f) => (bases.some((b, i) => width(`'${f}', ${b}`) !== base[i]) ? 1 : 0)).join("");
  } catch {
    return "";
  }
}

function media(q: string): boolean {
  try {
    return matchMedia(q).matches;
  } catch {
    return false;
  }
}

/** 存储配额按 2 的幂分档（GB），避免每次空间变化都变。 */
async function storageBucket(): Promise<string> {
  try {
    const est = await navigator.storage?.estimate?.();
    const q = est?.quota ?? 0;
    if (!q) return "";
    return String(2 ** Math.round(Math.log2(q / 2 ** 30)));
  } catch {
    return "";
  }
}

type UAData = {
  getHighEntropyValues?: (hints: string[]) => Promise<{
    model?: string;
    platformVersion?: string;
    fullVersionList?: Array<{ brand: string; version: string }>;
  }>;
};

async function uaHigh(): Promise<Parts> {
  const ua = (navigator as Navigator & { userAgentData?: UAData }).userAgentData;
  if (!ua?.getHighEntropyValues) return {};
  try {
    const v = await ua.getHighEntropyValues(["model", "platformVersion", "fullVersionList"]);
    return {
      uaModel: v.model ?? "",
      uaPlatformVersion: v.platformVersion ?? "",
      uaFullVersions: (v.fullVersionList ?? []).map((b) => `${b.brand}/${b.version}`).join(","),
    };
  } catch {
    return {};
  }
}

/** 补充项：语音 / 字体的哈希，色域、HDR、像素比、存储档位、UA-CH 高熵值。任何一项取不到就是空串。 */
export async function extras(): Promise<Parts> {
  const gamut = media("(color-gamut: rec2020)")
    ? "rec2020"
    : media("(color-gamut: p3)")
      ? "p3"
      : media("(color-gamut: srgb)")
        ? "srgb"
        : "";
  const fonts = fontsFp();
  const [voices, storage, high] = await Promise.all([voicesFp().catch(() => ""), storageBucket(), uaHigh()]);
  return {
    voices,
    fonts: fonts ? (await sha256hex(fonts).catch(() => "")).slice(0, 16) : "",
    gamut,
    hdr: media("(dynamic-range: high)") ? "high" : "standard",
    dpr: window.devicePixelRatio ?? "",
    storage,
    ...high,
  };
}

type BatteryManager = { level: number; charging: boolean };

/** 电量快照（支持 Battery API 的浏览器）：同一台设备上的不同浏览器读到的一样。 */
async function battery(): Promise<BatteryManager | null> {
  const nav = navigator as Navigator & { getBattery?: () => Promise<BatteryManager> };
  if (!nav.getBattery) return null;
  try {
    const b = await nav.getBattery();
    return typeof b.level === "number" ? { level: b.level, charging: !!b.charging } : null;
  } catch {
    return null;
  }
}

/**
 * 浏览器标记：服务端在 owl_bid Cookie 里种的长期随机 ID（32 位小写十六进制），这里在 localStorage 另存一份，
 * 只清 Cookie 时带回去，服务端照旧认同一个浏览器。存取不了（隐私模式等）就不带。
 */
const BID_KEY = "owl_bid";
const BID_RE = /^[0-9a-f]{32}$/;

function readBid(): string {
  try {
    const v = localStorage.getItem(BID_KEY) || "";
    return BID_RE.test(v) ? v : "";
  } catch {
    return "";
  }
}

function writeBid(v: string): void {
  try {
    localStorage.setItem(BID_KEY, v);
  } catch {
    /* 存不了就算了 */
  }
}

/** 本机设备指纹 id：页面打开后在空闲时算好，登录链接直接带上（点击时不再等待）。 */
export const deviceId = signal("");
let started = false;

/**
 * 提前算设备指纹：只算一次；把各项（连同 localStorage 里的浏览器标记）POST 到 /public/device-fp，
 * 服务端回的 id 写进 deviceId，回的浏览器标记存回 localStorage。
 * 提前算好，登录按钮就是普通链接，点下去直接跳；不用在点击时等指纹、再用脚本跳转。
 */
export function prepareDevice(): void {
  if (started || typeof window === "undefined") return;
  started = true;
  const run = async () => {
    try {
      const [data, extra, bat] = await Promise.all([fingerprint(), extras().catch(() => ({})), battery()]);
      const bid = readBid();
      const body: Record<string, unknown> = { data, extra };
      if (bid) body.bid = bid;
      if (bat) body.battery = bat;
      const r = await fetch("/public/device-fp", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(body),
      });
      if (r.status !== 200) return;
      const res = (await r.json()) as { id?: unknown; bid?: unknown };
      if (typeof res.bid === "string" && BID_RE.test(res.bid)) writeBid(res.bid);
      // 服务端算的 id：只留十六进制，最多 32 位；拿不到就不带 dev，登录照常
      const id = typeof res.id === "string" ? res.id.replace(/[^0-9a-f]/gi, "").slice(0, 32) : "";
      if (id) deviceId.value = id;
    } catch {
      /* 算不出来 / 发不出去就不带，登录照常 */
    }
  };
  const idle = (window as Window & { requestIdleCallback?: (cb: () => void) => void }).requestIdleCallback;
  if (idle) idle(() => void run());
  else setTimeout(() => void run(), 300);
}

/** 登录链接：带上已经算好的设备指纹（还没算好就是原链接）。是普通链接，点下去直接跳。 */
export function useLoginHref(base = "/login"): string {
  useEffect(() => prepareDevice(), []);
  const id = deviceId.value;
  return id ? `${base}?dev=${encodeURIComponent(id)}` : base;
}
