/**
 * 画匣（Beta）的数据层：模型 / 尺寸 / 采样器清单、请求体组装、免费档钳制。
 * 只有纯函数，不碰 DOM 和网络；组件在 Paintbox.tsx，收图（PNG / ZIP）在 paintbox-image.ts。
 * 口径与网关一致（gate-core policy）：免费档步数 ≤ 28、单张 ≤ 1024×1024；参数原样发送，本站不替成员补质量词。
 */
import type { MemberPolicy } from "../../lib/member-policy";

export type ModelFamily = "v5" | "v4" | "v3";
export type ModelOption = { id: string; label: string; family: ModelFamily };

/** 本站白名单里的常用模型；V5 需要 Key 单独开放（policy.v5），单独一组。 */
export const BASE_MODELS: ModelOption[] = [
  { id: "nai-diffusion-4-5-full", label: "V4.5 Full", family: "v4" },
  { id: "nai-diffusion-4-5-curated", label: "V4.5 Curated", family: "v4" },
  { id: "nai-diffusion-4-full", label: "V4 Full", family: "v4" },
  { id: "nai-diffusion-4-curated", label: "V4 Curated", family: "v4" },
  { id: "nai-diffusion-3", label: "V3", family: "v3" },
  { id: "nai-diffusion-furry-3", label: "Furry V3", family: "v3" },
];
export const V5_MODELS: ModelOption[] = [{ id: "nai-diffusion-5-full", label: "V5 Full", family: "v5" }];

/** 尺寸预设全部落在免费规格内（≤ 1024×1024 像素）；custom 用宽高输入框，走下面的钳制。 */
export const SIZE_PRESETS: Array<{ id: string; label: string; w?: number; h?: number }> = [
  { id: "portrait", label: "竖图 832×1216", w: 832, h: 1216 },
  { id: "landscape", label: "横图 1216×832", w: 1216, h: 832 },
  { id: "square", label: "方图 1024×1024", w: 1024, h: 1024 },
  { id: "tall", label: "高竖 768×1344", w: 768, h: 1344 },
  { id: "wide", label: "宽横 1344×768", w: 1344, h: 768 },
  { id: "p896", label: "896×1152", w: 896, h: 1152 },
  { id: "l896", label: "1152×896", w: 1152, h: 896 },
  { id: "custom", label: "自定义" },
];

/** 采样器按模型族分开：V4 / V4.5 / V5 不支持 plms / ddim / k_dpm_*（网关会拒绝，这里就不给选）。 */
export const SAMPLERS_V4 = [
  "k_euler_ancestral",
  "k_euler",
  "k_dpmpp_2m",
  "k_dpmpp_2s_ancestral",
  "k_dpmpp_sde",
];
export const SAMPLERS_V3 = ["k_euler_ancestral", "k_euler", "k_lms", "k_dpm_2", "k_dpm_2_ancestral", "ddim"];
export const NOISE_SCHEDULES = ["karras", "exponential", "polyexponential"];

export const FREE_MAX_PIXELS = 1024 * 1024;
export const FREE_MAX_STEPS = 28;
export const SCALE_MAX = 10;
export const CFG_RESCALE_MAX = 1;

export type PaintForm = {
  positive: string;
  negative: string;
  model: string;
  preset: string;
  width: number;
  height: number;
  steps: number;
  scale: number;
  sampler: string;
  noise: string;
  cfgRescale: number;
  /** 种子输入原文：空 / -1 = 随机，出图前换成真种子，出完能「用这颗种子再画」。 */
  seed: string;
  smea: boolean;
};

export const DEFAULT_FORM: PaintForm = {
  positive: "",
  negative: "",
  model: "nai-diffusion-4-5-full",
  preset: "portrait",
  width: 832,
  height: 1216,
  steps: 28,
  scale: 5,
  sampler: "k_euler_ancestral",
  noise: "karras",
  cfgRescale: 0,
  seed: "-1",
  smea: false,
};

const FALLBACK_MODEL: ModelOption = BASE_MODELS[0] ?? { id: "", label: "?", family: "v4" };
export const modelOf = (id: string): ModelOption =>
  [...BASE_MODELS, ...V5_MODELS].find((m) => m.id === id) ?? FALLBACK_MODEL;

export const isV3 = (id: string): boolean => modelOf(id).family === "v3";
export const isV5 = (id: string): boolean => modelOf(id).family === "v5";

/** V4 / V4.5 / V5 才有噪声调度和 CFG Rescale；V3 用 SMEA。 */
export const hasSchedule = (id: string): boolean => isV3(id) === false;

export function samplersFor(id: string): string[] {
  return isV3(id) ? SAMPLERS_V3 : SAMPLERS_V4;
}

export function sizeLabel(w: number, h: number): string {
  return `${w}×${h}`;
}

/** 随机种子：上游只接受 0..4294967295，超出范围网关会当参数错误，这里就按同口径生成。 */
export function randomSeed(): number {
  return Math.floor(Math.random() * 4294967296);
}

/** "-1" / 空 = 随机；其余按整数解析，坏输入退回随机（生成按钮前还会再校验一次提示词）。 */
export function resolveSeed(input: string): number {
  const t = input.trim();
  if (!t || t === "-1") return randomSeed();
  const n = Number(t);
  if (!Number.isFinite(n)) return randomSeed();
  const i = Math.trunc(n);
  return i >= 0 && i <= 4294967295 ? i : randomSeed();
}

/**
 * 免费档钳制（paid_parameters = false 时）：步数压到 28，自定义尺寸等比缩到 1M 像素内。
 * 网关也会钳，但这里先钳并写说明，成员看到的尺寸就是实际送出的尺寸，不是「送出去被改了」。
 */
export function clampForm(form: PaintForm, paid: boolean): { form: PaintForm; notes: string[] } {
  if (paid) return { form, notes: [] };
  const notes: string[] = [];
  const next = { ...form };
  if (next.steps > FREE_MAX_STEPS) {
    next.steps = FREE_MAX_STEPS;
    notes.push(`步数超出免费规格，已按 ${FREE_MAX_STEPS} 步送出`);
  }
  const w = Math.max(1, Math.trunc(next.width) || 1);
  const h = Math.max(1, Math.trunc(next.height) || 1);
  if (w * h > FREE_MAX_PIXELS) {
    const k = Math.sqrt(FREE_MAX_PIXELS / (w * h));
    next.width = Math.max(64, Math.floor((w * k) / 64) * 64);
    next.height = Math.max(64, Math.floor((h * k) / 64) * 64);
    notes.push(`尺寸超出免费规格，已缩到 ${sizeLabel(next.width, next.height)}`);
  }
  return { form: next, notes };
}

/** 出图请求体：与网关测试夹具同一个形状（negative_prompt，V4 系的结构化提示词由网关补齐）。
 *  seed 由调用方解析好传入（界面要把用掉的种子记进参数），缺省时才自己随机。 */
export function buildRequest(
  form: PaintForm,
  seed: number = resolveSeed(form.seed),
): Record<string, unknown> {
  const v3 = isV3(form.model);
  const parameters: Record<string, unknown> = {
    width: form.width,
    height: form.height,
    steps: form.steps,
    scale: form.scale,
    sampler: form.sampler,
    n_samples: 1,
    seed,
    negative_prompt: form.negative.trim(),
  };
  if (hasSchedule(form.model)) {
    parameters.noise_schedule = form.noise;
    parameters.cfg_rescale = form.cfgRescale;
    parameters.params_version = 3;
  }
  if (v3 && form.smea) {
    parameters.smea = true;
    parameters.smea_dyn = true;
  }
  return { input: form.positive.trim(), model: form.model, action: "generate", parameters };
}

/** 一张成图展示用的参数（历史和属性面板共用，原始表单已经冻结，不随后续编辑变）。 */
export type GenAttrs = {
  model: string;
  width: number;
  height: number;
  steps: number;
  scale: number;
  sampler: string;
  noise: string;
  seed: number;
  prompt: string;
  negative: string;
  seconds: number;
};

/** 提示词的行内校验：空提示词不发请求（发出去也是 400，浪费一次排队）。
 *  长度不在这里拦：真实的字节上限（51200）由网关掌握并给出准确说明。 */
export function promptProblem(form: PaintForm): string | null {
  if (!form.positive.trim()) return "先写点提示词吧，比如：1girl, sunset, very aesthetic";
  return null;
}

/**
 * 标签补全的返回解析：上游的形状可能进化，这里只认「字符串数组」和「{tag / name} 对象数组」，
 * 认不出来就回空数组，界面提示「这次没拿到」。
 */
export function parseSuggestions(body: unknown): string[] {
  const tags = (body as { tags?: unknown } | null)?.tags;
  if (!Array.isArray(tags)) return [];
  const out: string[] = [];
  for (const t of tags) {
    if (typeof t === "string" && t.trim()) out.push(t.trim());
    else if (t && typeof t === "object") {
      const s = (t as { tag?: unknown; name?: unknown }).tag ?? (t as { name?: unknown }).name;
      if (typeof s === "string" && s.trim()) out.push(s.trim());
    }
  }
  return [...new Set(out)].slice(0, 12);
}

/** 生成一次的计费口径，只做展示提示（实际记账以请求记录为准）。 */
export function costHint(model: string, paid: boolean): string {
  if (isV5(model)) return "计入今天的 V5 额度";
  if (paid) return "超出免费规格的部分按 Anlas 计费";
  return "计入今天的免费出图张数";
}

/** Key 的出图前置：功能没开 / 网页没登录过 / 没拿到 Key，分别给出能落地的提示。 */
export function entryProblem(me: {
  key?: string;
  requires_web_login?: boolean;
  policy?: MemberPolicy;
}): string | null {
  if (!me.key) return "还没有拿到这把 Key 的完整信息，刷新页面后再试。";
  if (me.requires_web_login) return "先在页面顶部用 Discord 登录一次，登录后才能出图。";
  if (me.policy?.image === false) return "出图功能没有开放给你的 Key，如有疑问请在 Discord 联系站长。";
  return null;
}
