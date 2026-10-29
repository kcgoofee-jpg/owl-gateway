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
      human: `${CURATED_HEAVY_UC}, @_@, mismatched pupils, glowing eyes, bad anatomy`,
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
  human: "人物重点",
  furry: "Furry 优先",
};

const UC_ORDER: UcPreset[] = ["heavy", "light", "human", "furry", "none"];

export function ucPresetList(model: string): UcPreset[] {
  const available = new Set(Object.keys(ucOptions(model)) as UcPreset[]);
  return UC_ORDER.filter((id) => available.has(id));
}

export type QualityPreset = "none" | "standard" | "light";

export function qualityPresetList(model: string): QualityPreset[] {
  return isV5(model) ? ["standard", "light", "none"] : ["standard", "none"];
}

export function qualityTags(model: string, preset: QualityPreset): string {
  if (preset === "none") return "";
  // V5 的「轻量」是官方单独给的短预设；其余情况按模型给标准质量词
  if (preset === "light" && isV5(model)) return "very aesthetic, amazing quality, no text";
  return QUALITY_TAGS[model] ?? "";
}

export function qualityPresetLabel(id: QualityPreset): string {
  return id === "none" ? "无" : id === "light" ? "轻量" : "标准";
}

/** 模型下拉的分组说明。文案自写，不搬 NovelAI 商标。 */
export type ModelBlurb = { group: "new" | "legacy"; blurb: string; caution?: boolean };

export const MODEL_BLURBS: Record<string, ModelBlurb> = {
  "nai-diffusion-5-full": { group: "new", blurb: "最新也是最强的模型。" },
  "nai-diffusion-5-curated": { group: "new", blurb: "最新模型，用精选图训练。适合流式预览。" },
  "nai-diffusion-4-5-full": { group: "legacy", blurb: "V4.5 完整模型。已不推荐。" },
  "nai-diffusion-4-5-curated": {
    group: "legacy",
    blurb: "用精选图训练的 V4.5。已不推荐。",
    caution: true,
  },
  "nai-diffusion-4-full": { group: "legacy", blurb: "V4 完整模型。已不推荐。" },
  "nai-diffusion-4-curated": { group: "legacy", blurb: "用精选图训练的 V4。已不推荐。" },
  "nai-diffusion-3": { group: "legacy", blurb: "上一代动漫模型。已不推荐。" },
  "nai-diffusion-furry-3": { group: "legacy", blurb: "上一代 Furry 模型。已不推荐。" },
};

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

export type RefImage = {
  id: string;
  name: string;
  dataUrl: string;
  b64: string;
  strength: number;
  extracted: number;
  /** 已按这个 informationExtracted 编码过，再改就要重新编码（+2 Anlas）。 */
  encodedAt?: number;
  /** encode-vibe 的返回；和 encodedAt 成对。 */
  encoded?: string;
  enabled?: boolean;
};
export type SourceImage = { name: string; dataUrl: string; b64: string };

/** 单张图（源图 / 蒙版 / 参考图）的体量上限：请求体网关限 25 MiB，base64 会膨胀 1/3，留出余量。 */
export const MAX_FILE_BYTES = 12 * 1024 * 1024;
export const MAX_REFS = 8;
export const MAX_PRECISE = 8;
export const MAX_CHARACTERS = 6;

export type RefPayload = {
  image: string;
  strength: number;
  extracted?: number;
  encoded?: string;
  encodedAt?: number;
};

export type PreciseCaption = "character" | "style" | "character&style";
export const PRECISE_CAPTIONS: Array<{ id: PreciseCaption; label: string }> = [
  { id: "character", label: "角色" },
  { id: "style", label: "风格" },
  { id: "character&style", label: "角色与风格" },
];

export type PrecisePayload = {
  image: string;
  caption: PreciseCaption;
  extracted: number;
  strength: number;
  fidelity: number;
};

export type CharacterGender = "none" | "female" | "male";

export type CharacterPayload = {
  prompt: string;
  uc: string;
  center?: { x: number; y: number };
  enabled?: boolean;
};

/** V3 发原图；V4 / V4.5 要先编码；V5 不支持 Vibe。 */
export function vibeWire(model: string): "raw" | "encoded" | "none" {
  if (isV3(model)) return "raw";
  if (model.startsWith("nai-diffusion-4")) return "encoded";
  return "none";
}

export function vibeEncodeBody(image: string, model: string, informationExtracted: number) {
  return { image, model, informationExtracted };
}

/** 编码模型先把每张图换成 encode-vibe 的返回；没开 Vibe 的模型直接丢掉。已编码且信息提取没改就复用。 */
export async function prepareVibeRefs(
  model: string,
  refs: RefPayload[],
  encode: (image: string, extracted: number) => Promise<string>,
): Promise<RefPayload[]> {
  const wire = vibeWire(model);
  if (!refs.length || wire === "none") return [];
  if (wire === "raw") {
    return refs.map(({ image, strength, extracted }) =>
      extracted != null ? { image, strength, extracted } : { image, strength },
    );
  }
  const out: RefPayload[] = [];
  for (const ref of refs) {
    const extracted = ref.extracted ?? ref.strength;
    const image =
      typeof ref.encoded === "string" && ref.encoded.length > 0 && ref.encodedAt === extracted
        ? ref.encoded
        : await encode(ref.image, extracted);
    out.push({ image, strength: ref.strength, extracted });
  }
  return out;
}

/**
 * 工作台的请求体：在 nai-config 的文生图请求上按模式补 action 与图参。
 * 图生图 = parameters.image + strength/noise；局部重绘 = image + mask + img2img.strength，
 * 且 add_original_image=false。精准参考和 Vibe 互斥，有精准参考时不发 Vibe。
 * V3 的 SMEA 用网关认识的 sm / sm_dyn。
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
    characters?: CharacterPayload[];
    precise?: PrecisePayload[];
    varietyPlus?: boolean;
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
  const precise = task.precise ?? [];
  if (precise.length) {
    p.director_reference_images = precise.map((r) => r.image);
    p.director_reference_descriptions = precise.map((r) => ({
      caption: { base_caption: r.caption, char_captions: [] },
      legacy_uc: false,
    }));
    p.director_reference_information_extracted = precise.map((r) => r.extracted);
    p.director_reference_strength_values = precise.map((r) => r.strength);
    p.director_reference_secondary_strength_values = precise.map((r) => r.fidelity);
  } else if (task.refs.length) {
    p.reference_image_multiple = task.refs.map((r) => r.image);
    p.reference_information_extracted_multiple = task.refs.map((r) => r.extracted ?? r.strength);
    p.reference_strength_multiple = task.refs.map((r) => r.strength);
  }
  const characters = (task.characters ?? [])
    .filter((c) => c.enabled !== false && c.prompt.trim())
    .slice(0, MAX_CHARACTERS);
  if (characters.length && task.form.model.startsWith("nai-diffusion-4")) {
    p.characterPrompts = characters.map((c) => {
      const row: Record<string, unknown> = { prompt: c.prompt.trim(), uc: c.uc.trim(), enabled: true };
      if (c.center) row.center = { x: c.center.x, y: c.center.y };
      return row;
    });
    if (characters.some((c) => c.center)) p.use_coords = true;
  }
  if (p.smea === true || p.smea_dyn === true) {
    p.sm = true;
    p.sm_dyn = true;
  }
  delete p.smea;
  delete p.smea_dyn;
  if (task.varietyPlus && !isV5(task.form.model)) {
    const sigma = varietySigma(task.form.model) * varietyScale(task.form.width, task.form.height);
    if (sigma > 0) p.skip_cfg_above_sigma = sigma;
  }
  if (isV5(task.form.model) && task.form.sampler === "k_euler_ancestral") {
    p.prefer_brownian = true;
    p.deliberate_euler_ancestral_bug = false;
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

/**
 * 官方 Prompt Randomizer：`||红发|蓝发||` 出图时随机取一项。
 * 同一颗种子也会换选项。空选项（单独一个逗号）会被清掉。
 */
export function expandPromptRandomizer(text: string): string {
  const picked = text.replace(/\|\|([\s\S]*?)\|\|/g, (_all, inner: string) => {
    const parts = inner.split("|");
    if (!parts.length) return "";
    return parts[Math.floor(Math.random() * parts.length)] ?? "";
  });
  return picked
    .replace(/(?:\s*,\s*)+/g, ", ")
    .replace(/^[,\s]+|[,\s]+$/g, "")
    .trim();
}

const pickOne = <T>(xs: readonly T[]): T => xs[Math.floor(Math.random() * xs.length)] as T;

const EYES = [
  "aqua eyes",
  "grey eyes",
  "blue eyes",
  "green eyes",
  "brown eyes",
  "red eyes",
  "sanpaku",
] as const;
const HAIR_LEN = [
  "short hair",
  "medium hair",
  "long hair",
  "bowl cut",
  "single hair bun",
  "ponytail",
] as const;
const HAIR_COLOR = [
  "black hair",
  "brown hair",
  "blonde hair",
  "silver hair",
  "dark blue hair",
  "light blue hair",
  "dark green hair",
  "multicolored hair",
] as const;
const CLOTHES_F = [
  "school uniform",
  "hoodie",
  "serafuku",
  "dress",
  "robe",
  "spandex",
  "waist apron",
  "jacket",
] as const;
const CLOTHES_M = ["hoodie", "jacket", "shirt", "coat", "armor", "robe"] as const;

/** 随机角色提示词（自有词库，不搬官方专有表）。 */
export function randomCharacterPrompt(gender: CharacterGender = "none"): {
  gender: CharacterGender;
  prompt: string;
} {
  const g: CharacterGender = gender === "none" ? pickOne(["female", "male"] as const) : gender;
  const who = g === "male" ? pickOne(["boy", "1boy", "man"] as const) : pickOne(["girl", "1girl"] as const);
  const tags = [
    who,
    pickOne(EYES),
    pickOne(HAIR_LEN),
    pickOne(HAIR_COLOR),
    g === "male"
      ? pickOne(["toned", "slender", "muscular"] as const)
      : pickOne(["flat chest", "small breasts", "medium breasts"] as const),
    g === "male" ? pickOne(CLOTHES_M) : pickOne(CLOTHES_F),
  ];
  return { gender: g, prompt: tags.join(", ") };
}

export function randomBasePrompt(): string {
  const n = pickOne([1, 1, 1, 2, 3] as const);
  const who = n === 1 ? pickOne(["1girl", "1boy", "1girl, solo"] as const) : n === 2 ? "2girls" : "3girls";
  return [
    who,
    pickOne(["looking at viewer", "from side", "upper body", "full body", "focus"] as const),
    pickOne(["outdoors", "indoors", "seaside", "classroom", "night", "simple background"] as const),
    pickOne(["smile", "serious", "blush", ""] as const),
  ]
    .filter(Boolean)
    .join(", ");
}

const COST_AREA = 2.951_823_174_884_865e-6;
const COST_AREA_STEP = 5.753_298_233_447_344e-7;
const VIBE_ENCODE_ANLAS = 2;
const PRECISE_ANLAS = 5;
const FREE_VIBE_COUNT = 4;

/** 官方客户端同口径的 Anlas 估算：规格内第一张免费，Vibe 编码 2，精准参考每张 5。 */
export function estimateAnlasCost(input: {
  width: number;
  height: number;
  steps: number;
  nSamples: number;
  paid: boolean;
  smea?: boolean;
  strength?: number;
  pendingVibeEncodes: number;
  vibeCount: number;
  preciseCount: number;
}): number {
  const pixels = Math.max(1, input.width) * Math.max(1, input.height);
  const steps = Math.max(1, input.steps);
  const samples = Math.min(4, Math.max(1, Math.trunc(input.nSamples) || 1));
  const smeaMul = input.smea ? 1.2 : 1;
  const strength = Number.isFinite(input.strength) ? Math.min(1, Math.max(0, input.strength ?? 1)) : 1;
  const base = Math.ceil(COST_AREA * pixels + COST_AREA_STEP * pixels * steps);
  const perImage = Math.max(2, Math.ceil(base * smeaMul * (strength || 1)));
  const inSpec = pixels <= 1024 * 1024 && steps <= 28;
  const freeFirst = inSpec && input.preciseCount === 0;
  const billable = samples - (freeFirst && input.paid ? 1 : 0);
  // 公益站：规格内走每日张数，Anlas 只记超额、编码和精准参考。
  const gen = inSpec ? (input.paid ? billable * perImage : 0) : perImage * samples;
  const vibe =
    input.pendingVibeEncodes * VIBE_ENCODE_ANLAS +
    Math.max(0, input.vibeCount - FREE_VIBE_COUNT) * VIBE_ENCODE_ANLAS;
  return gen + vibe + input.preciseCount * PRECISE_ANLAS * samples;
}

export function pendingVibeEncodes(
  refs: Array<{ extracted: number; encodedAt?: number }>,
  encodedWire: boolean,
): number {
  if (!encodedWire) return 0;
  return refs.filter((r) => r.encodedAt !== r.extracted).length;
}

export function tokenBudget(parts: string[]): { local: number; total: number } {
  const tokens = parts.map((p) => estimateTokens(p));
  return { local: tokens[0] ?? 0, total: tokens.reduce((a, b) => a + b, 0) };
}

/* ---------------- 分辨率 / 高级 / 增强 / 导演 ---------------- */

export const RESOLUTION_GROUPS: Array<{ id: string; label: string; sizes: Array<[number, number]> }> = [
  {
    id: "normal",
    label: "标准",
    sizes: [
      [832, 1216],
      [1024, 1024],
      [1216, 832],
    ],
  },
  {
    id: "large",
    label: "大图",
    sizes: [
      [1024, 1536],
      [1472, 1472],
      [1536, 1024],
    ],
  },
  {
    id: "wallpaper",
    label: "壁纸",
    sizes: [
      [1088, 1920],
      [1920, 1088],
    ],
  },
  {
    id: "small",
    label: "小图",
    sizes: [
      [512, 768],
      [640, 640],
      [768, 512],
    ],
  },
];

export function resolutionGroup(width: number, height: number): string {
  for (const group of RESOLUTION_GROUPS) {
    if (group.sizes.some(([w, h]) => w === width && h === height)) return group.id;
  }
  return "custom";
}

/** V5 没有噪声计划选择；V3 多一个 native。 */
export function noiseChoices(model: string): string[] {
  if (isV5(model)) return [];
  if (isV3(model)) return ["native", "karras", "exponential", "polyexponential"];
  return ["karras", "exponential", "polyexponential"];
}

/** Variety+ 的基线 sigma。V5 不用。口径来自 nai-workbench（MIT）的 modelSettings。 */
export function varietySigma(model: string): number {
  if (isV5(model)) return 0;
  return model.startsWith("nai-diffusion-4-5") ? 58 : 19;
}

export function varietyScale(width: number, height: number): number {
  return Math.sqrt((Math.floor(width / 8) * Math.floor(height / 8)) / (104 * 152));
}

const ENHANCE_LEVELS: Array<{ strength: number; noise: number }> = [
  { strength: 0.2, noise: 0 },
  { strength: 0.4, noise: 0 },
  { strength: 0.5, noise: 0 },
  { strength: 0.6, noise: 0 },
  { strength: 0.7, noise: 0.1 },
];

export function enhanceLevel(level: number): { strength: number; noise: number } {
  const i = Math.min(ENHANCE_LEVELS.length, Math.max(1, Math.trunc(level) || 1)) - 1;
  return ENHANCE_LEVELS[i] ?? { strength: 0.5, noise: 0 };
}

/** 增强倍数：64 对齐，且放大后不超过约 314 万像素。 */
export function enhanceScales(width: number, height: number): number[] {
  const out: number[] = [];
  for (const scale of [1, 1.5, 2]) {
    const w = width * scale;
    const h = height * scale;
    if (w * h <= 3_145_728 && w % 64 === 0 && h % 64 === 0) out.push(scale);
  }
  return out.length ? out : [1];
}

export function nextSeeds(seed: number, count: number): number[] {
  const out: number[] = [];
  let n = seed >>> 0;
  for (let i = 0; i < count; i++) {
    n = (n + 1) >>> 0;
    out.push(n);
  }
  return out;
}

export const DIRECTOR_TOOLS: Array<{ id: string; label: string; prompt: boolean }> = [
  { id: "bg-removal", label: "去除背景", prompt: false },
  { id: "lineart", label: "线稿", prompt: false },
  { id: "sketch", label: "草图", prompt: false },
  { id: "colorize", label: "上色", prompt: true },
  { id: "emotion", label: "表情", prompt: true },
  { id: "declutter", label: "清理杂物", prompt: false },
  { id: "declutter-keep-bubbles", label: "清理杂物（保留气泡）", prompt: false },
];

export const DIRECTOR_EMOTIONS: Array<{ id: string; label: string }> = [
  { id: "neutral", label: "自然" },
  { id: "happy", label: "开心" },
  { id: "sad", label: "难过" },
  { id: "angry", label: "生气" },
  { id: "surprised", label: "惊讶" },
  { id: "scared", label: "害怕" },
  { id: "excited", label: "兴奋" },
  { id: "shy", label: "害羞" },
  { id: "tired", label: "疲倦" },
  { id: "nervous", label: "紧张" },
  { id: "thinking", label: "思考" },
  { id: "confused", label: "困惑" },
  { id: "disgusted", label: "厌恶" },
  { id: "smug", label: "得意" },
  { id: "bored", label: "无聊" },
  { id: "laughing", label: "大笑" },
  { id: "irritated", label: "烦躁" },
  { id: "aroused", label: "动情" },
  { id: "embarrassed", label: "尴尬" },
  { id: "worried", label: "担忧" },
  { id: "love", label: "爱意" },
  { id: "determined", label: "坚定" },
  { id: "hurt", label: "受伤" },
  { id: "playful", label: "俏皮" },
];

/** 导演工具请求。表情写进 prompt：网关只转发 prompt 和 defry。 */
export function directorRequest(input: {
  image: string;
  tool: string;
  prompt: string;
  emotion: string;
  defry: number;
}): Record<string, unknown> {
  const body: Record<string, unknown> = { image: input.image, req_type: input.tool };
  if (input.tool === "colorize" || input.tool === "emotion") {
    const extra = input.prompt.trim();
    body.prompt = input.tool === "emotion" ? [input.emotion, extra].filter(Boolean).join(", ") : extra;
    body.defry = Math.min(5, Math.max(0, Math.trunc(input.defry) || 0));
  }
  return body;
}

export function upscaleRequest(image: string): Record<string, unknown> {
  return { image, scale: 2, model: "nai-diffusion-5-curated", declared_blur_sigma: 0 };
}

export function isPngDataUrl(url: string): boolean {
  return url.startsWith("data:image/png");
}
