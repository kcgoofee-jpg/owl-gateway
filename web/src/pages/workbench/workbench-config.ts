/**
 * 生图工作台的数据层：在 lib/nai-config（模型 / 尺寸 / 钳制 / 请求体）之上补工作台特有的一层——
 * 质量 / 负面预设组合（对齐官方客户端口径）、图生图与参考图的参数组装、五组提示词批量展开、
 * 批量队列的失败分类与限速重试、提示词 token 粗估。只有纯函数，不碰 DOM 和网络。
 * 预设文案取自 nai-workbench（MIT）的公开口径，按本站白名单模型改写。
 */
import { buildRequest, isV3, isV5, type PaintForm } from "../../lib/nai-config";

/* ---------------- 质量与负面预设 ---------------- */

/** 官方客户端的质量预设词（按模型）。 */
const QUALITY_TAGS: Record<string, string> = {
  "nai-diffusion-5-full": "very aesthetic, masterpiece, no text",
  "nai-diffusion-4-5-full": "very aesthetic, masterpiece, no text",
  "nai-diffusion-4-5-curated": "very aesthetic, masterpiece, no text, -0.8::feet::, rating:general",
  "nai-diffusion-4-full": "no text, best quality, very aesthetic, absurdres",
  "nai-diffusion-4-curated": "rating:general, best quality, very aesthetic, absurdres",
  "nai-diffusion-3": "best quality, amazing quality, very aesthetic, absurdres",
  "nai-diffusion-furry-3": "{best quality}, {amazing quality}",
};

const HEAVY_UC =
  "lowres, artistic error, film grain, scan artifacts, worst quality, bad quality, jpeg artifacts, very displeasing, chromatic aberration, dithering, halftone, screentone, multiple views, logo, too many watermarks, negative space, blank page";
const V5_LIGHT_UC =
  "lowres, bad hands, bad anatomy, artistic error, sepia, white haze, worst quality, very displeasing, jpeg artifacts, 0::ai-generated::";
const V45_LIGHT_UC =
  "lowres, artistic error, scan artifacts, worst quality, bad quality, jpeg artifacts, multiple views, very displeasing, too many watermarks, negative space, blank page";
const FURRY_UC =
  "{worst quality}, distracting watermark, unfinished, bad quality, {widescreen}, upscale, {sequence}, {{grandfathered content}}, blurred foreground, chromatic aberration, sketch, everyone, [sketch background], simple, [flat colors], ych (character), outline, multiple scenes, [[horror (theme)]], comic";
const HUMAN_UC = `${HEAVY_UC}, @_@, mismatched pupils, glowing eyes, bad anatomy`;
const CURATED_HEAVY_UC =
  "blurry, lowres, upscaled, artistic error, film grain, scan artifacts, worst quality, bad quality, jpeg artifacts, very displeasing, chromatic aberration, halftone, multiple views, logo, too many watermarks, negative space, blank page";
const V4_LIGHT_UC = "blurry, lowres, error, worst quality, bad quality, jpeg artifacts, very displeasing";
const V3_UC =
  "lowres, {bad}, error, fewer, extra, missing, worst quality, jpeg artifacts, bad quality, watermark, unfinished, displeasing, chromatic aberration, signature, extra digits, artistic error, username, scan, [abstract]";

export type UcPreset = "none" | "light" | "heavy" | "human" | "furry";

export function ucOptions(model: string): Partial<Record<UcPreset, string>> {
  if (isV5(model)) {
    return { none: "", light: V5_LIGHT_UC, heavy: HEAVY_UC, furry: FURRY_UC, human: HUMAN_UC };
  }
  if (model === "nai-diffusion-4-5-full") {
    return { none: "", light: V45_LIGHT_UC, heavy: HEAVY_UC, furry: FURRY_UC, human: HUMAN_UC };
  }
  if (model === "nai-diffusion-4-5-curated") {
    return {
      none: "",
      light:
        "blurry, lowres, upscaled, artistic error, scan artifacts, jpeg artifacts, logo, too many watermarks, negative space, blank page",
      heavy: CURATED_HEAVY_UC,
    };
  }
  if (model.startsWith("nai-diffusion-4")) {
    const curated = model.includes("curated");
    return {
      none: "",
      light: `${V4_LIGHT_UC}${curated ? ", logo, dated, signature" : ""}, white blank page, blank page`,
      heavy: `blurry, lowres, error, film grain, scan artifacts, worst quality, bad quality, jpeg artifacts, very displeasing, chromatic aberration${curated ? ", logo, dated, signature, multiple views, gigantic breasts" : ", multiple views, logo, too many watermarks"}, white blank page, blank page`,
    };
  }
  if (model.includes("furry")) {
    return {
      none: "lowres",
      light:
        "{worst quality}, guide lines, unfinished, bad, url, tall image, widescreen, compression artifacts, unknown text",
      heavy:
        "{{worst quality}}, [displeasing], {unusual pupils}, guide lines, {{unfinished}}, {bad}, url, artist name, {{tall image}}, mosaic, {sketch page}, comic panel, impact (font), [dated], {logo}, ych, {what}, {where is your god now}, {distorted text}, repeated text, {floating head}, {1994}, {widescreen}, absolutely everyone, sequence, {compression artifacts}, hard translated, {cropped}, {commissioner name}, unknown text, high contrast",
    };
  }
  return {
    none: "lowres",
    light: "lowres, jpeg artifacts, worst quality, watermark, blurry, very displeasing",
    heavy: V3_UC,
    human: `${V3_UC}, bad anatomy, bad hands, @_@, mismatched pupils, heart-shaped pupils, glowing eyes`,
  };
}

export const UC_LABELS: Record<UcPreset, string> = {
  none: "无",
  light: "轻量",
  heavy: "强力",
  human: "人物优先",
  furry: "Furry 优先",
};

export function ucPresetList(model: string): UcPreset[] {
  return Object.keys(ucOptions(model)) as UcPreset[];
}

export type QualityPreset = "none" | "standard" | "light";

export function qualityPresetList(model: string): QualityPreset[] {
  return isV5(model) ? ["none", "light", "standard"] : ["none", "standard"];
}

export function qualityTags(model: string, preset: QualityPreset): string {
  if (preset === "none") return "";
  // V5 的「轻量」是官方单独给的短预设；其余情况按模型给标准质量词
  if (preset === "light" && isV5(model)) return "very aesthetic, amazing quality, no text";
  return QUALITY_TAGS[model] ?? "";
}

function joinPrompt(...parts: string[]): string {
  return parts
    .map((s) => s.trim())
    .filter(Boolean)
    .join(", ");
}

/** 正面提示词组合：画师词最先，用户提示词居中，质量词收尾（与官方客户端顺序一致）。 */
export function composePositive(
  model: string,
  prompt: string,
  artist: string,
  preset: QualityPreset,
): string {
  return joinPrompt(artist, prompt, qualityTags(model, preset));
}

/**
 * 负面提示词组合：UC 预设在前、用户负面在后。官方客户端的两条口径照搬：
 * 非 Curated 模型选了预设且提示词没写 nsfw 时，负面自动带 nsfw；
 * V3 / Furry V3 不选预设且自己写了负面时，预设的 lowres 兜底让位（上游对空 UC 自带处理）。
 */
export function composeNegative(model: string, negative: string, preset: UcPreset, positive: string): string {
  const options = ucOptions(model);
  const presetValue = (options[preset] ?? "").trim();
  const userNeg = negative.trim();
  const v3 = isV3(model);
  const defaultNeg = v3 && preset === "none" && userNeg ? "" : presetValue;
  const nsfw =
    !model.includes("curated") && preset !== "none" && presetValue && !positive.toLowerCase().includes("nsfw")
      ? "nsfw"
      : "";
  return joinPrompt(nsfw, defaultNeg, userNeg);
}

/* ---------------- 模式 / 参考图 / 图生图 ---------------- */

export type WbMode = "generate" | "img2img" | "infill";

export const MODE_LABELS: Record<WbMode, string> = {
  generate: "文生图",
  img2img: "图生图",
  infill: "局部重绘",
};

/** 图生图 / 局部重绘走网关的 policy.img2img 开关；没开就只给文生图。 */
export function modesFor(policy: { img2img?: boolean } | undefined): WbMode[] {
  return policy?.img2img ? ["generate", "img2img", "infill"] : ["generate"];
}

export type RefImage = { id: string; name: string; dataUrl: string; b64: string; strength: number };
export type SourceImage = { name: string; dataUrl: string; b64: string };

/** 单张图（源图 / 蒙版 / 参考图）的体量上限：请求体网关限 25 MiB，base64 会膨胀 1/3，留出余量。 */
export const MAX_FILE_BYTES = 12 * 1024 * 1024;
export const MAX_REFS = 4;

export type RefPayload = { image: string; strength: number };

/**
 * 工作台的请求体：在 nai-config 的文生图请求上按模式补 action 与图参。
 * 图生图 = parameters.image + strength/noise；局部重绘 = image + mask + img2img.strength，
 * 且 add_original_image=false（与官方客户端一致）；参考图按 NAI 的三数组同发。
 */
export function buildTaskRequest(
  task: {
    form: PaintForm;
    mode: WbMode;
    source?: SourceImage | null;
    mask?: SourceImage | null;
    strength: number;
    noise: number;
    refs: RefPayload[];
  },
  seed: number,
): Record<string, unknown> {
  const body = buildRequest(task.form, seed);
  if (task.mode !== "generate") body.action = task.mode;
  const p = body.parameters as Record<string, unknown>;
  if (task.mode !== "generate") {
    p.image = task.source?.b64 ?? "";
    if (task.mode === "img2img") {
      p.strength = task.strength;
      p.noise = task.noise;
    } else {
      p.mask = task.mask?.b64 ?? "";
      p.img2img = { strength: task.strength };
      p.add_original_image = false;
    }
  }
  if (task.refs.length) {
    p.reference_image_multiple = task.refs.map((r) => r.image);
    p.reference_information_extracted_multiple = task.refs.map((r) => r.strength);
    p.reference_strength_multiple = task.refs.map((r) => r.strength);
  }
  return body;
}

/* ---------------- 五组提示词批量 ---------------- */

export type Strings = { artist: string; quality: string; negative: string };
export type UnifiedKey = keyof Strings;
export type BatchUnified = Record<UnifiedKey, boolean>;
export type BatchGroup = {
  id: string;
  prompt: string;
  artist: string;
  quality: string;
  negative: string;
  count: number;
  enabled: boolean;
};

export const MAX_GROUPS = 5;
export const MAX_PER_GROUP = 20;
/** 一次批量最多送出的张数：公益站的额度与上游节奏都经不起一次挂满 5×20。 */
export const MAX_BATCH_TASKS = 20;

export function defaultGroups(): BatchGroup[] {
  return Array.from({ length: MAX_GROUPS }, (_, i) => ({
    id: `g${i + 1}`,
    prompt: "",
    artist: "",
    quality: "",
    negative: "",
    count: 1,
    enabled: true,
  }));
}

/** 每组可单独关掉「用共用词」；关掉后用组内自己的词。 */
export function resolveStrings(shared: Strings, group: BatchGroup, unified: BatchUnified): Strings {
  return {
    artist: unified.artist ? shared.artist : group.artist,
    quality: unified.quality ? shared.quality : group.quality,
    negative: unified.negative ? shared.negative : group.negative,
  };
}

export const clampCount = (n: number): number => Math.min(MAX_PER_GROUP, Math.max(1, Math.trunc(n) || 1));

export type WbTask = { label: string; form: PaintForm };

/** 批量展开：跳过没勾选或没写提示词的组，每组 count 张，总量封顶 MAX_BATCH_TASKS。 */
export function batchTasks(
  base: PaintForm,
  presets: { qualityPreset: QualityPreset; ucPreset: UcPreset },
  shared: Strings,
  unified: BatchUnified,
  groups: BatchGroup[],
): WbTask[] {
  const out: WbTask[] = [];
  groups.forEach((g, i) => {
    const prompt = g.prompt.trim();
    if (!g.enabled || !prompt) return;
    const s = resolveStrings(shared, g, unified);
    const positive = composePositive(base.model, prompt, s.artist, presets.qualityPreset);
    const negative = composeNegative(base.model, s.negative, presets.ucPreset, positive);
    const count = clampCount(g.count);
    for (let n = 0; n < count && out.length < MAX_BATCH_TASKS; n++) {
      out.push({
        label: `第 ${i + 1} 组 · ${n + 1}/${count}`,
        form: { ...base, positive, negative },
      });
    }
  });
  return out;
}

/* ---------------- 批量队列：失败分类与限速重试 ---------------- */

export type FailureKind = "failed" | "unknown";
export type TaskFailure = { kind: FailureKind; message: string };

/** 返回 null = 可以自动重试（限速且明确没开始画）；其余都要停下来。 */
export function classifyFailure(
  status: number,
  err: { retryable?: boolean; executionUnknown?: boolean; message?: string },
): TaskFailure | null {
  if (status === 429 && err.retryable && err.executionUnknown !== true) return null;
  if (err.executionUnknown) {
    return {
      kind: "unknown",
      message: err.message ?? "结果待核对：网关不确定这次是否已经出图，请稍后在「我的请求」里核对。",
    };
  }
  return { kind: "failed", message: err.message ?? `出图没有成功（${status}）。` };
}

export const RETRY_MIN_SEC = 1;
export const RETRY_MAX_SEC = 3600;
export const RETRY_FALLBACK_SEC = 10;

export function clampRetryAfter(sec: number | null): number {
  if (sec === null || !Number.isFinite(sec)) return RETRY_FALLBACK_SEC;
  return Math.min(RETRY_MAX_SEC, Math.max(RETRY_MIN_SEC, Math.ceil(sec)));
}

/* ---------------- 提示词 token 粗估 ---------------- */

/** 各模型族的提示词上限（与官方客户端一致）：V5 走 Qwen、V4 系走 T5、V3 走 CLIP。 */
export function tokenLimit(model: string): number {
  return isV5(model) ? 1471 : isV3(model) ? 225 : 512;
}

/**
 * 粗估，不是官方 tokenizer：剥掉权重写法（1.2::tag::）和强调括号后，
 * 中日韩字符按 1 个 token、其余按 4 个字符 ≈ 1 个 token。只用来给「写多了没有」一个数量级参考。
 */
export function estimateTokens(text: string): number {
  const t = text.replace(/-?\d+(\.\d+)?::/g, "").replace(/[{}[\]]/g, "");
  let cjk = 0;
  let other = 0;
  for (const ch of t) {
    if (/[\u3000-\u9fff\u3040-\u30ff\uac00-\ud7af\uff00-\uffef]/.test(ch)) cjk++;
    else other++;
  }
  return cjk + Math.ceil(other / 4);
}
