/**
 * 绘图页：official 壳子里的左栏、画布和会话画廊。
 * 请求组装在 workbench-config，网络在 api.ts。
 */
import { useEffect, useRef, useState } from "preact/hooks";
import {
  BASE_MODELS,
  clampForm,
  costHint,
  hasSchedule,
  isV3,
  isV5,
  type PaintForm,
  resolveSeed,
  samplersFor,
  V5_MODELS,
} from "../../lib/nai-config";
import { toast } from "../../ui";
import { encodeVibe, fetchSuggestions, makeShot, postImages, postTool, type Shot } from "./api";
import { readImageFile } from "./files";
import {
  AnlasIcon,
  ArrowIcon,
  CheckIcon,
  ChevronIcon,
  CloseIcon,
  CopyIcon,
  DiceIcon,
  ExpandIcon,
  HistoryIcon,
  ImageIcon,
  InpaintIcon,
  KebabIcon,
  LandscapeIcon,
  PlayIcon,
  PlusIcon,
  PortraitIcon,
  RefreshIcon,
  SeedIcon,
  SparkleIcon,
  SquareIcon,
  SwapIcon,
  TokenIcon,
  TrashIcon,
  UploadIcon,
  VibeIcon,
} from "./Icons";
import { MaskEditor } from "./MaskEditor";
import { QUICKSTART, quickstartSrc } from "./quickstart";
import { reloadMe } from "./store";
import { decodeTokenIds, type TokenizedText, tokenizerLabel, tokenizeText } from "./tokenizer";
import {
  applyModelModePrompt,
  buildTaskRequest,
  type CharacterGender,
  type CharacterPayload,
  type CharacterPositionMode,
  characterLimit,
  composeNegative,
  composePositive,
  enhanceLevel,
  enhanceScales,
  estimateAnlasCost,
  expandPromptRandomizer,
  isPngDataUrl,
  MAX_PRECISE,
  MAX_REFS,
  MODE_LABELS,
  MODEL_BLURBS,
  modesFor,
  nextSeeds,
  noiseChoices,
  PRECISE_CAPTIONS,
  type PreciseCaption,
  pendingVibeEncodes,
  prepareVibeRefs,
  type QualityPreset,
  qualityPresetLabel,
  qualityPresetList,
  qualityTags,
  RESOLUTION_GROUPS,
  type RefImage,
  randomBasePrompt,
  randomCharacterPrompt,
  resolutionGroup,
  type SourceImage,
  snapCharacterCenter,
  tokenLimit,
  UC_LABELS,
  type UcPreset,
  ucPresetList,
  upscaleRequest,
  vibeEncodeBody,
  vibeWire,
  type WbMode,
} from "./workbench-config";

export type CharacterDraft = {
  id: string;
  name: string;
  prompt: string;
  uc: string;
  x: number;
  y: number;
  enabled: boolean;
  gender: CharacterGender;
  open: boolean;
  ucTab: boolean;
  fromRandom: boolean;
};

export type PreciseDraft = {
  id: string;
  name: string;
  dataUrl: string;
  b64: string;
  caption: PreciseCaption;
  extracted: number;
  strength: number;
  fidelity: number;
};

export type DrawState = {
  form: PaintForm;
  modelMode: "anime" | "furry";
  positionMode: CharacterPositionMode;
  qualityPreset: QualityPreset;
  ucPreset: UcPreset;
  mode: WbMode;
  artist: string;
  source: SourceImage | null;
  mask: SourceImage | null;
  strength: number;
  i2iNoise: number;
  refs: RefImage[];
  precise: PreciseDraft[];
  characters: CharacterDraft[];
  varietyPlus: boolean;
  suggest: { busy: boolean; tags: string[] };
};

const SAMPLER_LABEL: Record<string, string> = {
  k_euler_ancestral: "Euler Ancestral",
  k_euler: "Euler",
  k_dpmpp_2m: "DPM++ 2M",
  k_dpmpp_2s_ancestral: "DPM++ 2S Ancestral",
  k_dpmpp_sde: "DPM++ 2M SDE",
  k_lms: "LMS",
  k_dpm_2: "DPM2",
  k_dpm_2_ancestral: "DPM2 Ancestral",
  ddim: "DDIM",
};

const newId = () => `${Date.now()}-${Math.random().toString(36).slice(2, 7)}`;

const blankChar = (partial: Partial<CharacterDraft> = {}): CharacterDraft => ({
  id: newId(),
  name: "角色 1",
  prompt: "",
  uc: "",
  x: 0.5,
  y: 0.5,
  enabled: true,
  gender: "none",
  open: true,
  ucTab: false,
  fromRandom: false,
  ...partial,
});

const GENDER_MARK: Record<CharacterGender, string> = { female: "♀", male: "♂", none: "○" };

const clamp01 = (raw: string): number => Math.min(1, Math.max(0, Number(raw) || 0));

const vibeId = (ref: { id: string; name: string }): string => {
  const a = ref.name
    .replace(/\.[^.]+$/, "")
    .replace(/\s+/g, "")
    .slice(0, 6)
    .toLowerCase();
  return `${a || "vibe"}-${ref.id.slice(-6)}`;
};

const vibeStrengthSum = (refs: Array<{ strength: number; enabled?: boolean }>): number =>
  refs.filter((r) => r.enabled !== false).reduce((n, r) => n + r.strength, 0);

export function DrawPane(props: {
  hidden: boolean;
  apiKey: string;
  policyImg2img: boolean;
  policyVibe: boolean;
  policyUpscale: boolean;
  draw: DrawState;
  patch: (p: Partial<DrawState>) => void;
  patchForm: (p: Partial<PaintForm>) => void;
  paid: boolean;
  v5Open: boolean;
  tagsOpen: boolean;
  historyOpen: boolean;
  onToggleHistory: () => void;
  shots: Shot[];
  addShots: (shots: Shot[]) => void;
  openShot: (shot: Shot) => void;
  mobileResult: boolean;
  setMobileResult: (v: boolean) => void;
  autoDownload?: boolean;
  hideHistory?: boolean;
  transparentBg?: boolean;
  straightAlpha?: boolean;
  setTransparentBg?: (v: boolean) => void;
}) {
  const { draw, patch, patchForm, paid } = props;
  const abort = useRef<AbortController | null>(null);
  const [running, setRunning] = useState(false);
  const [elapsed, setElapsed] = useState(0);
  const [note, setNote] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [shot, setShot] = useState<Shot | null>(null);
  const [advanced, setAdvanced] = useState(false);
  const [enhanceOpen, setEnhanceOpen] = useState(false);
  const [enhanceScale, setEnhanceScale] = useState(1);
  const [enhanceMag, setEnhanceMag] = useState(3);
  const [count, setCount] = useState(1);
  const sampleCount = paid ? count : 1;
  const transparentBg = props.transparentBg === true;
  const [modelOpen, setModelOpen] = useState(false);
  const [promptFocus, setPromptFocus] = useState(false);
  const [randomMode, setRandomMode] = useState(false);
  const [resetAsk, setResetAsk] = useState(false);
  const [maskOpen, setMaskOpen] = useState(false);
  const [vibeMenu, setVibeMenu] = useState(false);
  const [promptTab, setPromptTab] = useState<"prompt" | "uc">("prompt");
  const [addGenderOpen, setAddGenderOpen] = useState(false);
  const [qualityOpen, setQualityOpen] = useState(false);
  const [ucOpen, setUcOpen] = useState(false);
  const [samplerOpen, setSamplerOpen] = useState(false);
  const [tokenizerOpen, setTokenizerOpen] = useState(false);
  const [tokenDraft, setTokenDraft] = useState("");
  const [tokenInputMode, setTokenInputMode] = useState<"text" | "ids">("text");
  const [tokenOutputMode, setTokenOutputMode] = useState<"text" | "ids">("text");
  const [tokenIdsDraft, setTokenIdsDraft] = useState("");
  const [tokenResult, setTokenResult] = useState<TokenizedText | null>(null);
  const [tokenBusy, setTokenBusy] = useState(false);
  const [tokenError, setTokenError] = useState<string | null>(null);

  useEffect(() => {
    if (!running) return;
    setElapsed(0);
    const t = setInterval(() => setElapsed((s) => s + 1), 1000);
    return () => clearInterval(t);
  }, [running]);

  useEffect(() => {
    if (!tokenizerOpen) return;
    let current = true;
    setTokenBusy(true);
    setTokenError(null);
    const result =
      tokenInputMode === "text"
        ? tokenizeText(draw.form.model, tokenDraft)
        : decodeTokenIds(draw.form.model, tokenIdsDraft);
    result
      .then((value) => {
        if (current) setTokenResult(value);
      })
      .catch((err: unknown) => {
        if (current) {
          setTokenResult(null);
          setTokenError(err instanceof Error ? err.message : "分词器暂时不可用。");
        }
      })
      .finally(() => {
        if (current) setTokenBusy(false);
      });
    return () => {
      current = false;
    };
  }, [tokenizerOpen, draw.form.model, tokenInputMode, tokenDraft, tokenIdsDraft]);

  useEffect(() => {
    if (!modelOpen && !vibeMenu && !addGenderOpen && !qualityOpen && !ucOpen && !samplerOpen) return;
    const close = () => {
      setModelOpen(false);
      setVibeMenu(false);
      setAddGenderOpen(false);
      setQualityOpen(false);
      setUcOpen(false);
      setSamplerOpen(false);
    };
    window.addEventListener("click", close);
    return () => window.removeEventListener("click", close);
  }, [modelOpen, vibeMenu, addGenderOpen, qualityOpen, ucOpen, samplerOpen]);

  const modes = modesFor({ img2img: props.policyImg2img });
  const models = props.v5Open ? [...BASE_MODELS, ...V5_MODELS] : BASE_MODELS;
  const model = draw.form.model;
  const v4 = model.startsWith("nai-diffusion-4");
  const v45 = model.startsWith("nai-diffusion-4-5");
  const wire = vibeWire(model);
  const showVibe = props.policyVibe && wire !== "none" && draw.precise.length === 0;
  const showPrecise = props.policyVibe && v45 && draw.refs.length === 0;
  const groupId = resolutionGroup(draw.form.width, draw.form.height);
  const group = RESOLUTION_GROUPS.find((g) => g.id === groupId) ?? null;
  const noises = noiseChoices(model);

  const chooseModel = (id: string) => {
    const samplers = samplersFor(id);
    const sampler = samplers.includes(draw.form.sampler)
      ? draw.form.sampler
      : (samplers[0] ?? "k_euler_ancestral");
    const ucs = ucPresetList(id);
    const qs = qualityPresetList(id);
    const choices = noiseChoices(id);
    patchForm({
      model: id,
      sampler,
      smea: isV3(id) ? draw.form.smea : false,
      noise: choices.includes(draw.form.noise) ? draw.form.noise : (choices[0] ?? "karras"),
    });
    patch({
      ucPreset: ucs.includes(draw.ucPreset) ? draw.ucPreset : "none",
      qualityPreset: qs.includes(draw.qualityPreset) ? draw.qualityPreset : "none",
      mode: modes.includes(draw.mode) ? draw.mode : "generate",
      modelMode:
        id === "nai-diffusion-furry-3" ? "furry" : id === "nai-diffusion-3" ? "anime" : draw.modelMode,
      varietyPlus: isV5(id) ? false : draw.varietyPlus,
      precise: id.startsWith("nai-diffusion-4-5") ? draw.precise : [],
      refs: vibeWire(id) === "none" ? [] : draw.refs,
    });
  };

  const pick = async (files: FileList | null, apply: (img: SourceImage) => void) => {
    const file = files?.[0];
    if (!file) return;
    try {
      apply(await readImageFile(file));
    } catch (e) {
      toast(e instanceof Error ? e.message : "这张图读不出来。", { error: true });
    }
  };

  const remember = (img: Shot) => {
    setShot(img);
    props.addShots([img]);
    props.setMobileResult(true);
    void reloadMe();
    if (props.autoDownload) {
      const a = document.createElement("a");
      a.href = img.url;
      a.download = `owl-${img.attrs.seed}.${img.ext}`;
      a.click();
    }
  };

  const runBody = async (body: Record<string, unknown>, attrs: Shot["attrs"]) => {
    const controller = new AbortController();
    abort.current = controller;
    setRunning(true);
    setError(null);
    try {
      const imgs = await postImages(body, props.apiKey, controller.signal);
      const first = imgs[0];
      if (!first) throw new Error("返回里没有图片。");
      remember(makeShot(first, { ...attrs, seconds: attrs.seconds }));
    } catch (e) {
      if (e instanceof DOMException && e.name === "AbortError") {
        setNote("已取消等待。如果网关这时已经开始画，这次仍会计入今天的额度——稍后可以在「我的请求」里核对。");
      } else {
        setError(e instanceof Error ? e.message : "没有成功，请稍后再试。");
      }
    } finally {
      abort.current = null;
      setRunning(false);
    }
  };

  const composed = () => {
    const positive = composePositive(
      model,
      expandPromptRandomizer(draw.form.positive),
      expandPromptRandomizer(draw.artist),
      draw.qualityPreset,
      transparentBg,
    );
    const modePrompt = applyModelModePrompt(model, positive, draw.modelMode);
    const negative = composeNegative(
      model,
      expandPromptRandomizer(draw.form.negative),
      draw.ucPreset,
      modePrompt,
    );
    return clampForm({ ...draw.form, positive: modePrompt, negative }, paid);
  };

  const characters = (): CharacterPayload[] =>
    draw.characters
      .filter((c) => c.enabled && expandPromptRandomizer(c.prompt).trim())
      .slice(0, characterLimit(model))
      .map((c) => ({
        prompt: expandPromptRandomizer(c.prompt),
        uc: expandPromptRandomizer(c.uc),
        enabled: true,
        ...(draw.positionMode === "custom" ? { center: snapCharacterCenter(model, { x: c.x, y: c.y }) } : {}),
      }));

  const generate = async () => {
    if (running) return;
    if (!draw.form.positive.trim() && !draw.characters.some((c) => c.enabled && c.prompt.trim())) {
      setError("先写点提示词吧，比如：1girl, sunset, very aesthetic");
      return;
    }
    if (draw.mode !== "generate" && !draw.source) {
      setError(
        draw.mode === "infill"
          ? "局部重绘要先传一张原图，再用画笔标出要重画的区域。"
          : "图生图要先传一张原图。点左边「图生图」旁的上传按钮。",
      );
      return;
    }
    if (draw.mode === "infill" && !draw.mask) {
      setError("局部重绘还要蒙版：点铅笔打开画笔，白色是要重画的地方，黑色保留。");
      return;
    }
    if (draw.precise.length && !v45) {
      setError("精准参考只支持 V4.5 Full / Curated。请先换模型，或清掉精准参考。");
      return;
    }
    if (showVibe && draw.refs.filter((r) => r.enabled !== false).length > MAX_REFS) {
      setError(`Vibe 最多 ${MAX_REFS} 张。多出来的先关掉或删掉。`);
      return;
    }
    const { form: clamped, notes } = composed();
    const send = clamped;
    setNote(notes.length ? notes.join("；") : null);
    const controller = new AbortController();
    abort.current = controller;
    setRunning(true);
    setError(null);
    const startedAt = Date.now();
    try {
      const activeRefs = draw.refs.filter((r) => r.enabled !== false);
      const refs = await prepareVibeRefs(
        send.model,
        activeRefs.map((r) => ({
          image: r.b64,
          strength: r.strength,
          extracted: r.extracted,
          ...(r.encoded ? { encoded: r.encoded } : {}),
          ...(r.encodedAt != null ? { encodedAt: r.encodedAt } : {}),
        })),
        async (image, extracted) => {
          try {
            return await encodeVibe(
              vibeEncodeBody(image, send.model, extracted),
              props.apiKey,
              controller.signal,
            );
          } catch (e) {
            const msg = e instanceof Error ? e.message : "编码失败";
            throw new Error(`Vibe 编码没有成功：${msg}。改了「信息提取」就要重新编码，每次 2 Anlas。`);
          }
        },
      );
      const seed = resolveSeed(send.seed);
      const body = buildTaskRequest(
        {
          form: send,
          mode: draw.mode,
          source: draw.source,
          mask: draw.mask,
          strength: draw.strength,
          noise: draw.i2iNoise,
          refs,
          characters: characters(),
          positionMode: draw.positionMode,
          transparentBackground: transparentBg && isV5(model),
          straightAlpha: props.straightAlpha === true,
          precise: draw.precise.map((r) => ({
            image: r.b64,
            caption: r.caption,
            extracted: r.extracted,
            strength: r.strength,
            fidelity: r.fidelity,
          })),
          varietyPlus: draw.varietyPlus,
        },
        seed,
      );
      (body.parameters as Record<string, unknown>).n_samples = sampleCount;
      const imgs = await postImages(body, props.apiKey, controller.signal);
      const first = imgs[0];
      if (!first) throw new Error("返回里没有图片。");
      if (wire === "encoded" && activeRefs.length) {
        const encodedById = new Map(activeRefs.map((r, i) => [r.id, refs[i]?.image]));
        patch({
          refs: draw.refs.map((r) => {
            const encoded = encodedById.get(r.id);
            return encoded ? { ...r, encoded, encodedAt: r.extracted } : r;
          }),
        });
      }
      remember(
        makeShot(first, {
          model: send.model,
          width: send.width,
          height: send.height,
          steps: send.steps,
          scale: send.scale,
          sampler: send.sampler,
          noise: send.noise,
          seed,
          prompt: send.positive.trim(),
          negative: send.negative.trim(),
          seconds: (Date.now() - startedAt) / 1000,
        }),
      );
    } catch (e) {
      if (e instanceof DOMException && e.name === "AbortError") {
        setNote("已取消等待。如果网关这时已经开始画，这次仍会计入今天的额度——稍后可以在「我的请求」里核对。");
      } else {
        setError(e instanceof Error ? e.message : "没有成功，请稍后再试。");
      }
    } finally {
      abort.current = null;
      setRunning(false);
    }
  };

  const useShotAs = (kind: "img2img" | "infill" | "vibe" | "precise") => {
    if (!shot) return;
    const img = { name: `shot-${shot.attrs.seed}.png`, dataUrl: shot.url, b64: shot.b64 };
    if (kind === "img2img" || kind === "infill") {
      patch({ mode: kind, source: img });
      props.setMobileResult(false);
      return;
    }
    if (kind === "vibe" && showVibe && draw.refs.length < MAX_REFS) {
      patch({
        refs: [...draw.refs, { id: newId(), ...img, strength: 0.6, extracted: 1, enabled: true }],
      });
    }
    if (kind === "precise" && showPrecise && isPngDataUrl(shot.url) && draw.precise.length < MAX_PRECISE) {
      patch({
        precise: [
          ...draw.precise,
          {
            id: newId(),
            ...img,
            caption: "character",
            extracted: 1,
            strength: 1,
            fidelity: 1,
          },
        ],
      });
    }
  };

  const variations = async () => {
    if (!shot || running || !props.policyImg2img) return;
    const { form: send } = composed();
    const seeds = nextSeeds(shot.attrs.seed, 3);
    for (const seed of seeds) {
      const body = buildTaskRequest(
        {
          form: { ...send, positive: shot.attrs.prompt, negative: shot.attrs.negative },
          mode: "img2img",
          source: { name: "variation", dataUrl: shot.url, b64: shot.b64 },
          strength: 0.8,
          noise: 0.1,
          refs: [],
        },
        seed,
      );
      const controller = new AbortController();
      abort.current = controller;
      setRunning(true);
      try {
        const imgs = await postImages(body, props.apiKey, controller.signal);
        const first = imgs[0];
        if (first) {
          remember(
            makeShot(first, {
              ...shot.attrs,
              seed,
              seconds: 0,
              width: send.width,
              height: send.height,
            }),
          );
        }
      } catch (e) {
        if (!(e instanceof DOMException && e.name === "AbortError")) {
          setError(e instanceof Error ? e.message : "变体没有成功。");
        }
        break;
      } finally {
        abort.current = null;
        setRunning(false);
      }
    }
  };

  const enhance = async () => {
    if (!shot || running) return;
    const { form: send } = composed();
    const mag = enhanceLevel(enhanceMag);
    const width = Math.round(shot.attrs.width * enhanceScale);
    const height = Math.round(shot.attrs.height * enhanceScale);
    const body = buildTaskRequest(
      {
        form: {
          ...send,
          width,
          height,
          positive: shot.attrs.prompt,
          negative: shot.attrs.negative,
        },
        mode: "img2img",
        source: { name: "enhance", dataUrl: shot.url, b64: shot.b64 },
        strength: mag.strength,
        noise: mag.noise,
        refs: [],
      },
      resolveSeed("-1"),
    );
    await runBody(body, {
      ...shot.attrs,
      width,
      height,
      seed: Number((body.parameters as { seed: number }).seed),
      seconds: 0,
    });
  };

  const upscale = async () => {
    if (!shot || running) return;
    const controller = new AbortController();
    abort.current = controller;
    setRunning(true);
    setError(null);
    try {
      const imgs = await postTool("/ai/upscale", upscaleRequest(shot.b64), props.apiKey, controller.signal);
      const first = imgs[0];
      if (!first) throw new Error("返回里没有图片。");
      remember(
        makeShot(first, {
          ...shot.attrs,
          width: shot.attrs.width * 2,
          height: shot.attrs.height * 2,
          seconds: 0,
        }),
      );
    } catch (e) {
      setError(e instanceof Error ? e.message : "放大没有成功。");
    } finally {
      abort.current = null;
      setRunning(false);
    }
  };

  const suggestTags = async () => {
    if (draw.suggest.busy || !props.apiKey) return;
    const prompt = draw.form.positive.trim();
    if (!prompt) {
      toast("先写几个开头（人物、场景都行），再点补全。");
      return;
    }
    patch({ suggest: { busy: true, tags: [] } });
    try {
      const tags = await fetchSuggestions(prompt, model, props.apiKey, new AbortController().signal);
      if (!tags.length) toast("这次没拿到标签建议，换个写法再试。");
      patch({ suggest: { busy: false, tags } });
    } catch (e) {
      toast(e instanceof Error ? e.message : "标签补全没有成功，请稍后再试。", { error: true });
      patch({ suggest: { busy: false, tags: [] } });
    }
  };

  const scales = shot ? enhanceScales(shot.attrs.width, shot.attrs.height) : [1];
  const currentModel = models.find((m) => m.id === model) ?? models[0];
  const qualityHint = qualityTags(model, draw.qualityPreset);
  const allPromptParts = [
    draw.form.positive,
    draw.artist,
    qualityHint,
    ...draw.characters.filter((c) => c.enabled).map((c) => c.prompt),
  ];
  const promptLocal = useModelTokenCount(model, draw.form.positive);
  const promptTotal = useModelTokenCount(model, allPromptParts.filter(Boolean).join(", "));
  const negativeCount = useModelTokenCount(model, draw.form.negative);
  const promptTokens = { local: promptLocal.count, total: promptTotal.count };
  const limit = tokenLimit(model);
  const promptTokenStatus = [promptLocal.status, promptTotal.status].includes("error")
    ? "分词器暂不可用"
    : [promptLocal.status, promptTotal.status].includes("loading")
      ? "正在读取分词器…"
      : `这段用了 ${promptTokens.local} 个 token，当前合计 ${promptTokens.total}。上限 ${limit}`;
  const vibePending = pendingVibeEncodes(
    draw.refs.filter((r) => r.enabled !== false),
    wire === "encoded",
  );
  const anlas = estimateAnlasCost({
    width: draw.form.width,
    height: draw.form.height,
    steps: draw.form.steps,
    nSamples: sampleCount,
    paid,
    smea: draw.form.smea,
    strength: draw.mode === "generate" ? 1 : draw.strength,
    pendingVibeEncodes: vibePending,
    vibeCount: draw.refs.length,
    preciseCount: draw.precise.length,
  });
  const seedDisplay = draw.form.seed === "-1" || draw.form.seed.trim() === "" ? "" : draw.form.seed;
  const patchChar = (id: string, next: Partial<CharacterDraft>) =>
    patch({ characters: draw.characters.map((c) => (c.id === id ? { ...c, ...next } : c)) });
  const moveChar = (i: number, dir: -1 | 1) => {
    const j = i + dir;
    if (j < 0 || j >= draw.characters.length) return;
    const next = [...draw.characters];
    const a = next[i];
    const b = next[j];
    if (!a || !b) return;
    next[i] = b;
    next[j] = a;
    patch({ characters: next });
  };
  const rollRandom = () => {
    const n = Math.min(characterLimit(model), 1 + Math.floor(Math.random() * 3));
    const chars = Array.from({ length: n }, (_, i) => {
      const rolled = randomCharacterPrompt();
      return blankChar({ ...rolled, name: `角色 ${i + 1}`, fromRandom: true, open: n === 1 });
    });
    patchForm({ positive: randomBasePrompt() });
    patch({ characters: v4 || isV5(model) ? chars : [] });
    setRandomMode(true);
  };
  const applyQuickstart = (prompt: string) => {
    if (props.v5Open && !isV5(model)) chooseModel("nai-diffusion-5-curated");
    patchForm({ positive: prompt });
    patch({ qualityPreset: "standard" });
    setPromptTab("prompt");
    toast("已复制提示词");
  };
  const resetAi = () => {
    patchForm({
      steps: 23,
      scale: isV5(model) ? 7 : 5,
      sampler: "k_euler_ancestral",
      seed: "-1",
      noise: noiseChoices(model)[0] ?? "karras",
      cfgRescale: 0,
      smea: false,
      width: 832,
      height: 1216,
      preset: "832x1216",
    });
    patch({ varietyPlus: false, qualityPreset: "standard", ucPreset: "heavy" });
    setCount(1);
    props.setTransparentBg?.(false);
    setAdvanced(false);
    setResetAsk(false);
  };

  return (
    <section
      class={props.mobileResult ? "nai-shell is-result" : "nai-shell"}
      role="tabpanel"
      hidden={props.hidden}
      aria-label="出图"
    >
      <aside class={advanced ? "nai-sidebar wb-side is-advanced" : "nai-sidebar wb-side"}>
        <div class="nai-scroll">
          <div class="nv-model">
            <div class="nv-model__pick">
              <span class="nv-model__k">模型</span>
              <button
                type="button"
                class="nv-model__btn"
                aria-haspopup="listbox"
                aria-expanded={modelOpen}
                onClick={(e) => {
                  e.stopPropagation();
                  setModelOpen((v) => !v);
                }}
              >
                <b>{currentModel?.label ?? "选择模型"}</b>
                <ChevronIcon size={14} />
              </button>
              {modelOpen && (
                <div class="nv-model__menu" role="listbox" aria-label="模型">
                  {(["new", "legacy"] as const).map((group) => {
                    const rows = models.filter((m) => (MODEL_BLURBS[m.id]?.group ?? "legacy") === group);
                    if (!rows.length) return null;
                    return (
                      <div key={group}>
                        <p class="nv-model__group">{group === "new" ? "新" : "旧"}</p>
                        {rows.map((m) => {
                          const meta = MODEL_BLURBS[m.id];
                          return (
                            <button
                              key={m.id}
                              type="button"
                              role="option"
                              aria-selected={m.id === model}
                              class={meta?.caution ? "is-caution" : undefined}
                              onClick={() => {
                                chooseModel(m.id);
                                setModelOpen(false);
                              }}
                            >
                              <b class="nv-model__name">{m.label}</b>
                              <span class="nv-model__blurb">{meta?.blurb ?? ""}</span>
                            </button>
                          );
                        })}
                      </div>
                    );
                  })}
                </div>
              )}
            </div>
            <button
              type="button"
              class="nv-mode-tag"
              aria-label={`切换模型模式，当前${draw.modelMode === "furry" ? "兽人" : "动漫"}`}
              title="模型模式会在提示词开头加入对应数据集标记"
              onClick={() => {
                if (isV3(model)) {
                  chooseModel(
                    model === "nai-diffusion-furry-3" ? "nai-diffusion-3" : "nai-diffusion-furry-3",
                  );
                } else {
                  patch({ modelMode: draw.modelMode === "furry" ? "anime" : "furry" });
                }
              }}
            >
              模式
              <SparkleIcon size={14} />
              {draw.modelMode === "furry" ? "兽人" : "动漫"}
            </button>
          </div>
          {modes.length > 1 && (
            <fieldset class="wb-modes nv-sr" aria-label="模式">
              {modes.map((m) => (
                <button
                  key={m}
                  type="button"
                  class={draw.mode === m ? "wb-mode wb-mode--on" : "wb-mode"}
                  aria-pressed={draw.mode === m}
                  onClick={() => patch({ mode: m })}
                >
                  {MODE_LABELS[m]}
                </button>
              ))}
            </fieldset>
          )}
          <div class={promptFocus ? "nv-prompt is-focus" : "nv-prompt"}>
            <div class="nv-prompt__head">
              <div class="nv-prompt__tabs" role="tablist" aria-label="提示词">
                <button
                  type="button"
                  role="tab"
                  class={promptTab === "prompt" ? "is-on" : undefined}
                  aria-selected={promptTab === "prompt"}
                  onClick={() => setPromptTab("prompt")}
                >
                  {randomMode ? "随机提示词" : draw.characters.length ? "基础提示词" : "提示词"}
                </button>
                <button
                  type="button"
                  role="tab"
                  class={promptTab === "uc" ? "is-on" : undefined}
                  aria-selected={promptTab === "uc"}
                  onClick={() => setPromptTab("uc")}
                >
                  负面内容
                </button>
                {promptTab === "uc" && (
                  <button
                    type="button"
                    class="nv-icon nv-icon--tiny"
                    aria-label="分词器"
                    title="分词器"
                    onClick={() => {
                      setTokenDraft(draw.form.negative);
                      setTokenInputMode("text");
                      setTokenOutputMode("text");
                      setTokenIdsDraft("");
                      setTokenizerOpen(true);
                    }}
                  >
                    <TokenIcon size={14} />
                  </button>
                )}
              </div>
              <span class="nv-prompt__tools">
                {randomMode ? (
                  <>
                    <button
                      type="button"
                      class="nv-icon"
                      aria-label="退出随机"
                      onClick={() => setRandomMode(false)}
                    >
                      <CloseIcon />
                    </button>
                    <button type="button" class="nv-icon" aria-label="换一组随机提示" onClick={rollRandom}>
                      <RefreshIcon />
                    </button>
                  </>
                ) : (
                  <>
                    {promptTab === "prompt" && (
                      <button
                        type="button"
                        class="nv-icon"
                        aria-label="分词器"
                        title="分词器"
                        onClick={() => {
                          setTokenDraft(draw.form.positive);
                          setTokenInputMode("text");
                          setTokenOutputMode("text");
                          setTokenIdsDraft("");
                          setTokenizerOpen(true);
                        }}
                      >
                        <TokenIcon />
                      </button>
                    )}
                    <button
                      type="button"
                      class="nv-icon"
                      aria-label="随机提示词"
                      title="随机提示词"
                      onClick={rollRandom}
                    >
                      <DiceIcon />
                    </button>
                  </>
                )}
                <button
                  type="button"
                  class="nv-icon"
                  aria-label="放大提示词"
                  aria-pressed={promptFocus}
                  onClick={() => setPromptFocus((v) => !v)}
                >
                  <ExpandIcon />
                </button>
                {props.tagsOpen && promptTab === "prompt" && (
                  <button
                    type="button"
                    class="nv-icon"
                    aria-label="标签补全"
                    title="标签补全"
                    onClick={() => void suggestTags()}
                  >
                    <SparkleIcon />
                    <span class="nv-sr">标签补全</span>
                  </button>
                )}
              </span>
            </div>
            {promptTab === "prompt" ? (
              <>
                <textarea
                  class="nai-textarea"
                  value={draw.form.positive}
                  placeholder="英文标签，逗号分隔"
                  aria-label="提示词"
                  onInput={(e) => patchForm({ positive: e.currentTarget.value })}
                />
                <div class="nv-pillrow nv-pillrow--end">
                  {isV5(model) && props.setTransparentBg && (
                    <button
                      type="button"
                      class={transparentBg ? "nv-pill is-on" : "nv-pill"}
                      aria-pressed={transparentBg}
                      aria-label="透明背景"
                      onClick={() => props.setTransparentBg?.(!transparentBg)}
                    >
                      透明背景
                    </button>
                  )}
                  <span class="nv-bubble">
                    <div class="nv-pop">
                      <button
                        type="button"
                        class="nv-pill"
                        aria-haspopup="listbox"
                        aria-expanded={qualityOpen}
                        aria-label="质量预设"
                        onClick={(e) => {
                          e.stopPropagation();
                          setQualityOpen((v) => !v);
                          setUcOpen(false);
                        }}
                      >
                        质量标签: {qualityPresetLabel(draw.qualityPreset)}
                      </button>
                      {qualityOpen && (
                        <div class="nv-pop__menu nv-pop__menu--up" role="listbox" aria-label="质量预设">
                          {qualityPresetList(model).map((id) => (
                            <button
                              key={id}
                              type="button"
                              role="option"
                              aria-selected={id === draw.qualityPreset}
                              onClick={() => {
                                patch({ qualityPreset: id });
                                setQualityOpen(false);
                              }}
                            >
                              {qualityPresetLabel(id)}
                            </button>
                          ))}
                        </div>
                      )}
                    </div>
                    {qualityHint ? (
                      <i class="nv-bubble__tip" role="tooltip">
                        会追加到提示词末尾：{qualityHint}
                      </i>
                    ) : null}
                  </span>
                </div>
                <div class="nv-bar" title={promptTokenStatus}>
                  <span
                    style={{
                      width: `${promptLocal.status === "ready" ? Math.min(100, Math.round((promptTokens.local / limit) * 100)) : 0}%`,
                    }}
                  />
                </div>
                <input
                  class="nv-artist input"
                  aria-label="画师词"
                  value={draw.artist}
                  placeholder="画师词（可选）"
                  onInput={(e) => patch({ artist: e.currentTarget.value })}
                />
                {draw.suggest.tags.length > 0 && (
                  <div class="nai-suggest">
                    {draw.suggest.tags.map((tag) => (
                      <button
                        key={tag}
                        type="button"
                        onClick={() => patchForm({ positive: appendTag(draw.form.positive, tag) })}
                      >
                        {tag}
                      </button>
                    ))}
                  </div>
                )}
              </>
            ) : (
              <>
                <textarea
                  class="nai-textarea"
                  aria-label="负面提示词"
                  value={draw.form.negative}
                  onInput={(e) => patchForm({ negative: e.currentTarget.value })}
                />
                <div class="nv-pillrow nv-pillrow--end">
                  <div class="nv-pop nv-pop--up">
                    <button
                      type="button"
                      class="nv-pill"
                      aria-haspopup="listbox"
                      aria-expanded={ucOpen}
                      aria-label="负面预设"
                      onClick={(e) => {
                        e.stopPropagation();
                        setUcOpen((v) => !v);
                        setQualityOpen(false);
                      }}
                    >
                      负面预设: {UC_LABELS[draw.ucPreset]}
                    </button>
                    {ucOpen && (
                      <div class="nv-pop__menu nv-pop__menu--up" role="listbox" aria-label="负面预设">
                        {ucPresetList(model).map((id) => (
                          <button
                            key={id}
                            type="button"
                            role="option"
                            aria-selected={id === draw.ucPreset}
                            onClick={() => {
                              patch({ ucPreset: id });
                              setUcOpen(false);
                            }}
                          >
                            {UC_LABELS[id]}
                          </button>
                        ))}
                      </div>
                    )}
                  </div>
                </div>
                <div
                  class="nv-bar"
                  aria-hidden="true"
                  title={
                    negativeCount.status === "error"
                      ? "分词器暂不可用"
                      : `${negativeCount.count} tokens / ${limit}`
                  }
                >
                  <span
                    style={{
                      width: `${negativeCount.status === "ready" ? Math.min(100, Math.round((negativeCount.count / tokenLimit(model)) * 100)) : 0}%`,
                    }}
                  />
                </div>
              </>
            )}
          </div>
          {(v4 || isV5(model)) && (
            <div class="nv-chars">
              <div class="nv-chars__head">
                <div>
                  <b>角色提示词</b>
                  <p>为画面中的角色单独写提示词。</p>
                </div>
                <div class="nv-pop">
                  <button
                    type="button"
                    class="nv-plusbtn"
                    aria-label="添加角色"
                    aria-expanded={addGenderOpen}
                    disabled={draw.characters.length >= characterLimit(model)}
                    onClick={(e) => {
                      e.stopPropagation();
                      setAddGenderOpen((v) => !v);
                    }}
                  >
                    <PlusIcon size={16} />
                  </button>
                  {addGenderOpen && (
                    <div class="nv-pop__menu" role="menu" aria-label="角色性别">
                      {(["female", "male", "none"] as const).map((g) => (
                        <button
                          key={g}
                          type="button"
                          role="menuitem"
                          onClick={() => {
                            patch({
                              characters: [
                                ...draw.characters,
                                blankChar({ gender: g, name: `角色 ${draw.characters.length + 1}` }),
                              ],
                            });
                            setAddGenderOpen(false);
                          }}
                        >
                          {GENDER_MARK[g]} {g === "female" ? "女" : g === "male" ? "男" : "其他"}
                        </button>
                      ))}
                    </div>
                  )}
                </div>
              </div>
              {draw.characters.some((c) => c.fromRandom) && (
                <button
                  type="button"
                  class="nv-clear-random"
                  onClick={() => patch({ characters: draw.characters.filter((c) => !c.fromRandom) })}
                >
                  清空随机角色
                </button>
              )}
              <fieldset class="nv-seg" aria-label="角色位置">
                <legend>位置</legend>
                <button
                  type="button"
                  aria-pressed={draw.positionMode === "ai"}
                  onClick={() => patch({ positionMode: "ai" })}
                >
                  AI 自动
                </button>
                <button
                  type="button"
                  aria-pressed={draw.positionMode === "custom"}
                  onClick={() => patch({ positionMode: "custom" })}
                >
                  自定义
                </button>
              </fieldset>
              {draw.characters.map((c, i) => (
                <div key={c.id} class={c.open ? "nv-char is-open" : "nv-char"}>
                  <div class="nv-char__bar">
                    <input
                      class="nv-char__name"
                      aria-label={`角色 ${i + 1} 名称`}
                      value={c.name}
                      onInput={(e) => patchChar(c.id, { name: e.currentTarget.value })}
                    />
                    <button
                      type="button"
                      class="nv-icon"
                      aria-label="上移"
                      disabled={i === 0}
                      onClick={() => moveChar(i, -1)}
                    >
                      <ArrowIcon dir="up" />
                    </button>
                    <button
                      type="button"
                      class="nv-icon"
                      aria-label="下移"
                      disabled={i === draw.characters.length - 1}
                      onClick={() => moveChar(i, 1)}
                    >
                      <ArrowIcon dir="down" />
                    </button>
                    <button
                      type="button"
                      class={c.enabled ? "nv-icon is-on" : "nv-icon"}
                      aria-pressed={c.enabled}
                      aria-label="启用角色"
                      onClick={() => patchChar(c.id, { enabled: !c.enabled })}
                    >
                      <CheckIcon />
                    </button>
                    <button
                      type="button"
                      class="nv-icon"
                      aria-label="复制角色提示词"
                      onClick={() => void navigator.clipboard?.writeText(c.prompt)}
                    >
                      <CopyIcon />
                    </button>
                    <button
                      type="button"
                      class="nv-icon"
                      aria-label="删除角色"
                      onClick={() => patch({ characters: draw.characters.filter((x) => x.id !== c.id) })}
                    >
                      <TrashIcon />
                    </button>
                    <button
                      type="button"
                      class="nv-icon"
                      aria-expanded={c.open}
                      aria-label={c.open ? "收起角色" : "展开角色"}
                      onClick={() => patchChar(c.id, { open: !c.open })}
                    >
                      <ExpandIcon />
                    </button>
                  </div>
                  {c.open ? (
                    <>
                      <div class="nv-tabs">
                        <button
                          type="button"
                          aria-pressed={!c.ucTab}
                          onClick={() => patchChar(c.id, { ucTab: false })}
                        >
                          提示词
                        </button>
                        <button
                          type="button"
                          aria-pressed={c.ucTab}
                          onClick={() => patchChar(c.id, { ucTab: true })}
                        >
                          负面内容
                        </button>
                      </div>
                      <textarea
                        class="nai-textarea"
                        aria-label={c.ucTab ? `角色 ${i + 1} 负面` : `角色 ${i + 1} 提示词`}
                        value={c.ucTab ? c.uc : c.prompt}
                        onInput={(e) =>
                          patchChar(
                            c.id,
                            c.ucTab ? { uc: e.currentTarget.value } : { prompt: e.currentTarget.value },
                          )
                        }
                      />
                      <TokenLengthBar model={model} text={c.ucTab ? c.uc : c.prompt} limit={limit} />
                      {draw.positionMode === "custom" && (
                        <div class="nai-footgrid">
                          <label class="nai-label">
                            X
                            <input
                              class="nai-input"
                              type="number"
                              min={0}
                              max={1}
                              step={0.05}
                              value={c.x}
                              onInput={(e) => patchChar(c.id, { x: Number(e.currentTarget.value) })}
                            />
                          </label>
                          <label class="nai-label">
                            Y
                            <input
                              class="nai-input"
                              type="number"
                              min={0}
                              max={1}
                              step={0.05}
                              value={c.y}
                              onInput={(e) => patchChar(c.id, { y: Number(e.currentTarget.value) })}
                            />
                          </label>
                        </div>
                      )}
                    </>
                  ) : (
                    <p class="nv-char__preview">{c.prompt || "还没写提示词。"}</p>
                  )}
                </div>
              ))}
            </div>
          )}
          {(props.policyImg2img || showVibe || showPrecise) && <p class="nv-res__kicker">参考图</p>}
          {props.policyImg2img && (
            <div class={draw.source ? "nv-action is-on" : "nv-action"}>
              <span class="nv-action__icon">
                <ImageIcon />
              </span>
              <button
                type="button"
                class="nv-action__copy"
                aria-label="图生图"
                aria-pressed={draw.mode !== "generate"}
                onClick={() => patch({ mode: draw.mode === "generate" ? "img2img" : "generate" })}
              >
                <b>图生图</b>
                <span>把这张图变成新图。</span>
              </button>
              <div class="nv-action__btns">
                <label class="nv-iconbtn nai-file" title="上传原图">
                  <UploadIcon />
                  <span class="nv-sr">上传原图</span>
                  <input
                    type="file"
                    accept="image/png,image/jpeg,image/webp"
                    onChange={(e) => {
                      void pick(e.currentTarget.files, (img) =>
                        patch({ mode: "img2img", source: img, mask: null }),
                      );
                      e.currentTarget.value = "";
                    }}
                  />
                </label>
                <button
                  type="button"
                  class="nv-iconbtn"
                  title="局部重绘"
                  aria-label="局部重绘"
                  onClick={() => {
                    if (!draw.source) {
                      toast("先上传原图，再画蒙版。");
                      return;
                    }
                    setMaskOpen(true);
                  }}
                >
                  <InpaintIcon size={16} />
                </button>
              </div>
            </div>
          )}
          {props.policyImg2img && draw.source && (
            <div class="nai-card nv-extra">
              <img class="nv-thumb" src={draw.source.dataUrl} alt="" width={72} height={72} />
              {draw.mask && <p class="nai-note">已有蒙版。白=重画，黑=保留。</p>}
              <div class="nv-slide">
                <span>强度</span>
                <input
                  type="number"
                  min={0.01}
                  max={0.99}
                  step={0.01}
                  aria-label="图生图强度"
                  value={draw.strength}
                  onInput={(e) =>
                    patch({ strength: Math.min(0.99, Math.max(0.01, Number(e.currentTarget.value) || 0.01)) })
                  }
                />
                <input
                  type="range"
                  min={0.01}
                  max={0.99}
                  step={0.01}
                  value={draw.strength}
                  onInput={(e) => patch({ strength: Number(e.currentTarget.value) })}
                />
              </div>
              {draw.mode !== "infill" && (
                <div class="nv-slide">
                  <span>噪声</span>
                  <input
                    type="number"
                    min={0}
                    max={0.99}
                    step={0.01}
                    aria-label="图生图噪声"
                    value={draw.i2iNoise}
                    onInput={(e) => patch({ i2iNoise: clamp01(e.currentTarget.value) })}
                  />
                  <input
                    type="range"
                    min={0}
                    max={0.99}
                    step={0.01}
                    value={draw.i2iNoise}
                    onInput={(e) => patch({ i2iNoise: Number(e.currentTarget.value) })}
                  />
                </div>
              )}
              <button
                type="button"
                class="nai-btn"
                onClick={() => patch({ source: null, mask: null, mode: "generate" })}
              >
                移除原图
              </button>
            </div>
          )}
          {showVibe && (
            <div class="nv-action nv-vibe">
              <span class="nv-action__icon">
                <VibeIcon />
              </span>
              <div class="nv-action__copy">
                <b>Vibe 迁移{draw.refs.length ? ` (${draw.refs.length})` : ""}</b>
                <span>改画面，留住构图。</span>
              </div>
              <div class="nv-action__btns">
                {draw.refs.length > 0 && (
                  <div class="nv-pop">
                    <button
                      type="button"
                      class="nv-iconbtn"
                      aria-label="Vibe 菜单"
                      aria-expanded={vibeMenu}
                      onClick={(e) => {
                        e.stopPropagation();
                        setVibeMenu((v) => !v);
                      }}
                    >
                      <KebabIcon />
                    </button>
                    {vibeMenu && (
                      <div class="nv-pop__menu">
                        <button
                          type="button"
                          onClick={() => {
                            const n = draw.refs.filter((r) => r.enabled !== false).length || 1;
                            const share = Math.round((1 / n) * 100) / 100;
                            patch({
                              refs: draw.refs.map((r) =>
                                r.enabled === false ? r : { ...r, strength: share },
                              ),
                            });
                            setVibeMenu(false);
                          }}
                        >
                          强度均分到 1
                        </button>
                        <button
                          type="button"
                          onClick={() => {
                            patch({ refs: [] });
                            setVibeMenu(false);
                          }}
                        >
                          清空全部
                        </button>
                      </div>
                    )}
                  </div>
                )}
                <label class="nv-iconbtn nai-file" title="添加 Vibe">
                  <PlusIcon size={16} />
                  <span class="nv-sr">添加 Vibe</span>
                  <input
                    type="file"
                    accept="image/png,image/jpeg,image/webp"
                    multiple
                    onChange={(e) => {
                      const room = MAX_REFS - draw.refs.length;
                      const list = [...(e.currentTarget.files ?? [])].slice(0, room);
                      e.currentTarget.value = "";
                      if (!list.length) {
                        toast(`Vibe 最多 ${MAX_REFS} 张。`);
                        return;
                      }
                      void Promise.all(list.map(readImageFile))
                        .then((imgs) =>
                          patch({
                            refs: [
                              ...draw.refs,
                              ...imgs.map((img) => ({
                                id: newId(),
                                ...img,
                                strength: 0.6,
                                extracted: 1,
                                enabled: true,
                              })),
                            ],
                          }),
                        )
                        .catch((err: unknown) =>
                          toast(err instanceof Error ? err.message : "图片没读出来。", { error: true }),
                        );
                    }}
                  />
                </label>
              </div>
            </div>
          )}
          {showVibe &&
            draw.refs.map((ref) => {
              const needEncode = wire === "encoded" && (ref.encodedAt !== ref.extracted || !ref.encoded);
              return (
                <div key={ref.id} class={ref.enabled === false ? "nv-vibe-card is-off" : "nv-vibe-card"}>
                  <img src={ref.dataUrl} alt="" />
                  <div>
                    <div class="nv-vibe-card__meta">
                      <b title={ref.name}>{vibeId(ref)}</b>
                      {needEncode && <span class="nv-costchip">2</span>}
                      <button
                        type="button"
                        class={ref.enabled === false ? "nv-icon" : "nv-icon is-on"}
                        aria-pressed={ref.enabled !== false}
                        aria-label="启用 Vibe"
                        onClick={() =>
                          patch({
                            refs: draw.refs.map((x) =>
                              x.id === ref.id ? { ...x, enabled: x.enabled === false } : x,
                            ),
                          })
                        }
                      >
                        <CheckIcon />
                      </button>
                      <button
                        type="button"
                        class="nv-icon"
                        aria-label="移除 Vibe"
                        onClick={() => patch({ refs: draw.refs.filter((x) => x.id !== ref.id) })}
                      >
                        <TrashIcon />
                      </button>
                    </div>
                    <div class="nv-slide">
                      <span>信息提取</span>
                      <input
                        type="number"
                        min={0}
                        max={1}
                        step={0.05}
                        aria-label={`${vibeId(ref)} 信息提取`}
                        value={ref.extracted}
                        onInput={(e) =>
                          patch({
                            refs: draw.refs.map((x) =>
                              x.id === ref.id ? { ...x, extracted: clamp01(e.currentTarget.value) } : x,
                            ),
                          })
                        }
                      />
                      <input
                        type="range"
                        min={0}
                        max={1}
                        step={0.05}
                        value={ref.extracted}
                        onInput={(e) =>
                          patch({
                            refs: draw.refs.map((x) =>
                              x.id === ref.id ? { ...x, extracted: Number(e.currentTarget.value) } : x,
                            ),
                          })
                        }
                      />
                    </div>
                    <div class="nv-slide">
                      <span>参考强度</span>
                      <input
                        type="number"
                        min={0}
                        max={1}
                        step={0.05}
                        aria-label={`${vibeId(ref)} 参考强度`}
                        value={ref.strength}
                        onInput={(e) =>
                          patch({
                            refs: draw.refs.map((x) =>
                              x.id === ref.id ? { ...x, strength: clamp01(e.currentTarget.value) } : x,
                            ),
                          })
                        }
                      />
                      <input
                        type="range"
                        min={0}
                        max={1}
                        step={0.05}
                        value={ref.strength}
                        onInput={(e) =>
                          patch({
                            refs: draw.refs.map((x) =>
                              x.id === ref.id ? { ...x, strength: Number(e.currentTarget.value) } : x,
                            ),
                          })
                        }
                      />
                    </div>
                    {needEncode && (
                      <p class="nv-vibe-warn">
                        下次生成需要编码，编码按 2 Anlas 计。改信息提取也会重新编码。
                      </p>
                    )}
                  </div>
                </div>
              );
            })}
          {showVibe && vibeStrengthSum(draw.refs) > 1.01 && (
            <p class="nai-note">
              已启用 Vibe 的参考强度合计 {vibeStrengthSum(draw.refs).toFixed(2)}，超过 1
              画面容易脏。菜单里可以均分。
            </p>
          )}
          {showPrecise && (
            <div class="nv-action">
              <span class="nv-action__icon">
                <ImageIcon />
              </span>
              <div class="nv-action__copy">
                <b>精准参考</b>
                <span>给角色或风格加一张参考图。</span>
              </div>
              <div class="nv-action__btns">
                <label class="nv-iconbtn nai-file" title="添加 PNG">
                  <UploadIcon />
                  <span class="nv-sr">添加 PNG</span>
                  <input
                    type="file"
                    accept="image/png"
                    onChange={(e) => {
                      void pick(e.currentTarget.files, (img) => {
                        if (!isPngDataUrl(img.dataUrl)) {
                          toast("精准参考只接受 PNG。", { error: true });
                          return;
                        }
                        const next: PreciseDraft = {
                          id: newId(),
                          name: img.name,
                          dataUrl: img.dataUrl,
                          b64: img.b64,
                          caption: "character",
                          extracted: 1,
                          strength: 1,
                          fidelity: 1,
                        };
                        patch({ precise: [...draw.precise, next].slice(0, MAX_PRECISE) });
                      });
                      e.currentTarget.value = "";
                    }}
                  />
                </label>
              </div>
            </div>
          )}
          {showPrecise && draw.precise.length > 0 && (
            <div class="nai-card nv-extra">
              {draw.precise.map((ref) => (
                <div key={ref.id}>
                  <img src={ref.dataUrl} alt="" width={52} height={52} />
                  <label class="nai-label">
                    类型
                    <select
                      class="nai-select"
                      value={ref.caption}
                      onChange={(e) =>
                        patch({
                          precise: draw.precise.map((x) =>
                            x.id === ref.id ? { ...x, caption: e.currentTarget.value as PreciseCaption } : x,
                          ),
                        })
                      }
                    >
                      {PRECISE_CAPTIONS.map((c) => (
                        <option key={c.id} value={c.id}>
                          {c.label}
                        </option>
                      ))}
                    </select>
                  </label>
                  <div class="nv-slide">
                    <span>参考强度</span>
                    <input
                      type="number"
                      min={0}
                      max={1}
                      step={0.05}
                      aria-label="精准参考强度"
                      value={ref.strength}
                      onInput={(e) =>
                        patch({
                          precise: draw.precise.map((x) =>
                            x.id === ref.id ? { ...x, strength: clamp01(e.currentTarget.value) } : x,
                          ),
                        })
                      }
                    />
                    <input
                      type="range"
                      min={0}
                      max={1}
                      step={0.05}
                      value={ref.strength}
                      onInput={(e) =>
                        patch({
                          precise: draw.precise.map((x) =>
                            x.id === ref.id ? { ...x, strength: Number(e.currentTarget.value) } : x,
                          ),
                        })
                      }
                    />
                  </div>
                  <div class="nv-slide">
                    <span>保真度</span>
                    <input
                      type="number"
                      min={0}
                      max={1}
                      step={0.05}
                      aria-label="精准参考保真度"
                      value={ref.fidelity}
                      onInput={(e) =>
                        patch({
                          precise: draw.precise.map((x) =>
                            x.id === ref.id ? { ...x, fidelity: clamp01(e.currentTarget.value) } : x,
                          ),
                        })
                      }
                    />
                    <input
                      type="range"
                      min={0}
                      max={1}
                      step={0.05}
                      value={ref.fidelity}
                      onInput={(e) =>
                        patch({
                          precise: draw.precise.map((x) =>
                            x.id === ref.id ? { ...x, fidelity: Number(e.currentTarget.value) } : x,
                          ),
                        })
                      }
                    />
                  </div>
                  <button
                    type="button"
                    class="nai-btn"
                    onClick={() => patch({ precise: draw.precise.filter((x) => x.id !== ref.id) })}
                  >
                    移除
                  </button>
                </div>
              ))}
            </div>
          )}
          <div class="nv-res">
            <span class="nv-res__kicker">图像设置</span>
            <div class="nv-res__top">
              <span>分辨率</span>
              <span class="nv-res__size">
                <input
                  aria-label="宽"
                  type="number"
                  min={64}
                  max={2048}
                  step={64}
                  value={draw.form.width}
                  onInput={(e) =>
                    patchForm({ preset: "custom", width: Math.max(64, Number(e.currentTarget.value) || 64) })
                  }
                />
                <button
                  type="button"
                  class="nv-icon"
                  aria-label="交换宽高"
                  title="交换宽高"
                  onClick={() =>
                    patchForm({
                      preset: "custom",
                      width: draw.form.height,
                      height: draw.form.width,
                    })
                  }
                >
                  <SwapIcon />
                </button>
                <input
                  aria-label="高"
                  type="number"
                  min={64}
                  max={2048}
                  step={64}
                  value={draw.form.height}
                  onInput={(e) =>
                    patchForm({ preset: "custom", height: Math.max(64, Number(e.currentTarget.value) || 64) })
                  }
                />
              </span>
            </div>
            <div class="nv-res__row">
              <select
                aria-label="分辨率"
                value={groupId === "custom" ? "custom" : groupId}
                onChange={(e) => {
                  const id = e.currentTarget.value;
                  if (id === "custom") {
                    patchForm({ preset: "custom" });
                    return;
                  }
                  const next = RESOLUTION_GROUPS.find((g) => g.id === id);
                  const size = next?.sizes[0];
                  if (size) patchForm({ preset: `${size[0]}x${size[1]}`, width: size[0], height: size[1] });
                }}
              >
                {RESOLUTION_GROUPS.map((g) => (
                  <option key={g.id} value={g.id}>
                    {g.label}
                  </option>
                ))}
                <option value="custom">自定义</option>
              </select>
              {(group?.sizes ?? []).map(([w, h]) => (
                <button
                  key={`${w}x${h}`}
                  type="button"
                  aria-pressed={draw.form.width === w && draw.form.height === h}
                  aria-label={w < h ? "竖图" : w > h ? "横图" : "方图"}
                  onClick={() => patchForm({ preset: `${w}x${h}`, width: w, height: h })}
                >
                  {w < h ? <PortraitIcon /> : w > h ? <LandscapeIcon /> : <SquareIcon />}
                </button>
              ))}
            </div>
            <div class="nv-count">
              <span>图片数量</span>
              <div>
                {[1, 2, 3, 4].map((n) => (
                  <button
                    key={n}
                    type="button"
                    aria-pressed={sampleCount === n}
                    disabled={!paid && n > 1}
                    onClick={() => setCount(n)}
                  >
                    {n}
                  </button>
                ))}
              </div>
            </div>
          </div>
        </div>
        <footer class="nai-foot">
          {advanced ? (
            <div class="nv-ai">
              <div class="nv-ai__head">
                <span class="nv-ai__title">AI 设置</span>
                <button type="button" class="nv-icon" aria-label="重置设置" onClick={() => setResetAsk(true)}>
                  <RefreshIcon />
                </button>
                <button
                  type="button"
                  class="nv-more"
                  aria-expanded="true"
                  aria-label="收起高级参数"
                  onClick={() => setAdvanced(false)}
                >
                  <ChevronIcon open />
                </button>
              </div>
              <label class="nv-slider">
                <span class="nv-slider__lab">
                  步数
                  <input
                    class="nv-slider__num"
                    type="number"
                    min={1}
                    max={50}
                    aria-label="步数"
                    value={draw.form.steps}
                    onInput={(e) => patchForm({ steps: Number(e.currentTarget.value) })}
                  />
                </span>
                <input
                  type="range"
                  min={1}
                  max={50}
                  value={draw.form.steps}
                  onInput={(e) => patchForm({ steps: Number(e.currentTarget.value) })}
                />
              </label>
              <label class="nv-slider">
                <span class="nv-slider__lab">
                  提示词引导
                  {!isV5(model) && (
                    <button
                      type="button"
                      class={draw.varietyPlus ? "nv-pill is-on" : "nv-pill"}
                      aria-pressed={draw.varietyPlus}
                      onClick={() => patch({ varietyPlus: !draw.varietyPlus })}
                    >
                      Variety+
                    </button>
                  )}
                  <input
                    class="nv-slider__num"
                    type="number"
                    min={0}
                    max={10}
                    step={0.1}
                    aria-label="引导"
                    value={draw.form.scale}
                    onInput={(e) => patchForm({ scale: Number(e.currentTarget.value) })}
                  />
                </span>
                <input
                  type="range"
                  min={0}
                  max={10}
                  step={0.1}
                  value={draw.form.scale}
                  onInput={(e) => patchForm({ scale: Number(e.currentTarget.value) })}
                />
              </label>
              <div class="nv-ai__row">
                <label>
                  种子
                  <span class="nv-seedrow">
                    <input
                      aria-label="种子"
                      placeholder="输入种子"
                      value={seedDisplay}
                      onInput={(e) => patchForm({ seed: e.currentTarget.value.trim() || "-1" })}
                    />
                    <button
                      type="button"
                      class="nv-seedbtn"
                      aria-label={shot ? "使用当前图的种子" : "随机种子"}
                      title={shot ? "使用当前图的种子" : "随机种子"}
                      onClick={() => patchForm({ seed: shot ? String(shot.attrs.seed) : "-1" })}
                    >
                      <SeedIcon />
                    </button>
                  </span>
                </label>
                <SamplerPick
                  model={model}
                  value={draw.form.sampler}
                  open={samplerOpen}
                  onToggle={() => setSamplerOpen((v) => !v)}
                  onPick={(s) => {
                    patchForm({ sampler: s });
                    setSamplerOpen(false);
                  }}
                />
              </div>
              <details class="nv-adv" open>
                <summary>高级设置</summary>
                {noises.length > 0 && (
                  <label class="nai-label">
                    噪声计划
                    <select
                      class="nai-select"
                      value={draw.form.noise}
                      onChange={(e) => patchForm({ noise: e.currentTarget.value })}
                    >
                      {noises.map((n) => (
                        <option key={n} value={n}>
                          {n === "karras" ? "karras（推荐）" : n}
                        </option>
                      ))}
                    </select>
                  </label>
                )}
                {hasSchedule(model) && (
                  <label class="nai-label">
                    提示词引导重缩放
                    <input
                      class="nai-input"
                      type="number"
                      min={0}
                      max={1}
                      step={0.01}
                      value={draw.form.cfgRescale}
                      onInput={(e) => patchForm({ cfgRescale: Number(e.currentTarget.value) })}
                    />
                  </label>
                )}
                {isV3(model) && (
                  <label class="nai-range">
                    <input
                      type="checkbox"
                      checked={draw.form.smea}
                      onChange={(e) => patchForm({ smea: e.currentTarget.checked })}
                    />
                    SMEA
                  </label>
                )}
              </details>
            </div>
          ) : (
            <div class="nv-stats">
              <label title="采样步数。步数越高细节越多，也越慢。">
                步数
                <input
                  type="number"
                  min={1}
                  max={50}
                  aria-label="步数"
                  value={draw.form.steps}
                  onInput={(e) => patchForm({ steps: Number(e.currentTarget.value) })}
                />
              </label>
              <label title="提示词引导强度。越高越贴提示词，也越容易过锐。">
                引导
                <input
                  type="number"
                  min={0}
                  max={10}
                  step={0.1}
                  aria-label="引导"
                  value={draw.form.scale}
                  onInput={(e) => patchForm({ scale: Number(e.currentTarget.value) })}
                />
              </label>
              <label title="种子。空表示每次随机。同一颗种子和同一套参数会画出同一张。">
                种子
                <span class="nv-seedrow">
                  <input
                    aria-label="种子"
                    placeholder=""
                    value={seedDisplay}
                    onInput={(e) => patchForm({ seed: e.currentTarget.value.trim() || "-1" })}
                  />
                  <button
                    type="button"
                    class="nv-seedbtn"
                    aria-label={shot ? "使用当前图的种子" : "随机种子"}
                    title={shot ? "使用当前图的种子" : "随机种子"}
                    onClick={() => patchForm({ seed: shot ? String(shot.attrs.seed) : "-1" })}
                  >
                    <SeedIcon />
                  </button>
                </span>
              </label>
              <SamplerPick
                compact
                model={model}
                value={draw.form.sampler}
                open={samplerOpen}
                onToggle={() => setSamplerOpen((v) => !v)}
                onPick={(s) => {
                  patchForm({ sampler: s });
                  setSamplerOpen(false);
                }}
              />
              <button
                type="button"
                class="nv-more"
                aria-expanded={false}
                aria-label="打开 AI 设置"
                onClick={() => setAdvanced(true)}
              >
                <PlayIcon size={14} />
              </button>
            </div>
          )}
          {resetAsk && (
            <div class="nv-reset" role="dialog" aria-label="重置设置">
              <p>把全部设置恢复成默认？</p>
              <button type="button" class="nai-generate nv-reset-yes" onClick={resetAi}>
                确定
              </button>
              <button type="button" class="nai-btn" onClick={() => setResetAsk(false)}>
                取消
              </button>
            </div>
          )}
          <button
            type="button"
            class="nai-generate"
            disabled={running}
            title="按当前提示词和参数出图。会计入今天的额度。"
            onClick={() => void generate()}
          >
            <span>{running ? "生成中…" : `生成 ${sampleCount} 张`}</span>
            <span class="nv-costchip">
              {anlas}
              <AnlasIcon size={12} />
              <span class="nv-sr">Anlas</span>
            </span>
          </button>
          <p class="nai-cost nv-sr">计入今天的额度。{costHint(model, paid)}。刷新即清空。</p>
        </footer>
      </aside>
      {tokenizerOpen && (
        <div class="nv-token" role="dialog" aria-label="分词器" aria-modal="true">
          <button
            type="button"
            class="nv-set-bg"
            aria-label="关闭分词器"
            onClick={() => setTokenizerOpen(false)}
          />
          <div class="nv-token__card">
            <header>
              <h2>分词器</h2>
              <button
                type="button"
                class="nv-iconbtn"
                aria-label="关闭分词器"
                onClick={() => setTokenizerOpen(false)}
              >
                <CloseIcon />
              </button>
            </header>
            <p>
              当前图像模型使用 <b>{tokenizerLabel(model)}</b>。提示词在本地切分，不会发送给 NovelAI。
            </p>
            <div class="nv-tabs" role="tablist" aria-label="分词器输入类型">
              <button
                type="button"
                role="tab"
                aria-selected={tokenInputMode === "text"}
                onClick={() => setTokenInputMode("text")}
              >
                文本输入
              </button>
              <button
                type="button"
                role="tab"
                aria-selected={tokenInputMode === "ids"}
                onClick={() => setTokenInputMode("ids")}
              >
                ID 输入
              </button>
            </div>
            <textarea
              class="nai-textarea"
              aria-label={tokenInputMode === "text" ? "分词文本" : "分词 ID"}
              placeholder={
                tokenInputMode === "text"
                  ? "在这里输入文字，下面会显示切分结果。"
                  : "输入空格或逗号分隔的 token ID。"
              }
              value={tokenInputMode === "text" ? tokenDraft : tokenIdsDraft}
              onInput={(e) =>
                tokenInputMode === "text"
                  ? setTokenDraft(e.currentTarget.value)
                  : setTokenIdsDraft(e.currentTarget.value)
              }
            />
            <div class="nv-tabs" role="tablist" aria-label="分词器输出类型">
              <button
                type="button"
                role="tab"
                aria-selected={tokenOutputMode === "text"}
                onClick={() => setTokenOutputMode("text")}
              >
                Token 文本
              </button>
              <button
                type="button"
                role="tab"
                aria-selected={tokenOutputMode === "ids"}
                onClick={() => setTokenOutputMode("ids")}
              >
                Token ID
              </button>
            </div>
            <div class="nv-token__pieces" aria-live="polite">
              {tokenBusy ? (
                <span class="nv-token__status">正在读取分词器…</span>
              ) : tokenError ? (
                <span class="nv-token__error">{tokenError}</span>
              ) : tokenResult ? (
                tokenOutputMode === "ids" ? (
                  <span>{tokenResult.ids.join(" ")}</span>
                ) : (
                  tokenResult.tokens.map((token, i) => (
                    <span class="nv-token__piece" key={`${i}-${token}`}>
                      {token}
                    </span>
                  ))
                )
              ) : null}
            </div>
            <p class="nv-token__stat">
              Token：{tokenResult?.ids.length ?? 0}　字符：
              {tokenInputMode === "text" ? tokenDraft.length : (tokenResult?.text.length ?? 0)}
            </p>
          </div>
        </div>
      )}
      {maskOpen && draw.source && (
        <MaskEditor
          source={draw.source}
          mask={draw.mask}
          width={draw.form.width}
          height={draw.form.height}
          onSave={(mask) => {
            patch({ mask, mode: "infill" });
            setMaskOpen(false);
          }}
          onClose={() => setMaskOpen(false)}
        />
      )}
      <section class="nai-main" aria-label="出图结果">
        <div class="nai-canvas-bar">
          <button
            type="button"
            class="nai-stage-toggle"
            aria-pressed={!props.mobileResult}
            onClick={() => props.setMobileResult(false)}
          >
            编辑
          </button>
          <button
            type="button"
            class="nai-stage-toggle"
            aria-pressed={props.mobileResult}
            onClick={() => props.setMobileResult(true)}
          >
            结果
          </button>
          {shot && (
            <button type="button" onClick={() => props.openShot(shot)}>
              大图
            </button>
          )}
          {shot && props.policyImg2img && (
            <>
              <button type="button" onClick={() => useShotAs("img2img")}>
                用作图生图
              </button>
              <button type="button" onClick={() => useShotAs("infill")}>
                重绘
              </button>
              <button type="button" onClick={() => void variations()}>
                三张变体
              </button>
              <button type="button" onClick={() => setEnhanceOpen((v) => !v)}>
                增强
              </button>
            </>
          )}
          {shot && showVibe && (
            <button type="button" onClick={() => useShotAs("vibe")}>
              用作 Vibe
            </button>
          )}
          {shot && showPrecise && (
            <button type="button" onClick={() => useShotAs("precise")}>
              用作精准参考
            </button>
          )}
          {shot && props.policyUpscale && (
            <button type="button" onClick={() => void upscale()}>
              放大 2 倍
            </button>
          )}
          {running && (
            <button type="button" onClick={() => abort.current?.abort()}>
              取消等待
            </button>
          )}
          {!props.hideHistory && (
            <button
              type="button"
              class="nai-histbtn"
              aria-pressed={props.historyOpen}
              aria-label={props.historyOpen ? "收起历史" : "展开历史"}
              title="刷新即清空"
              onClick={props.onToggleHistory}
            >
              <HistoryIcon />
            </button>
          )}
        </div>
        {error && <p class="nai-status nai-status--bad">{error}</p>}
        {note && !error && <p class="nai-status">{note}</p>}
        {running && <p class="nai-status">已等待 {elapsed} 秒</p>}
        <div class="wb-shot">
          {shot ? (
            <button
              type="button"
              class="nai-shotbtn"
              onClick={() => props.openShot(shot)}
              aria-label="查看成图"
            >
              <img src={shot.url} alt={`成图 ${shot.attrs.seed}`} />
            </button>
          ) : (
            <QuickstartGallery onPick={applyQuickstart} />
          )}
        </div>
        {enhanceOpen && shot && (
          <div class="nai-enhance">
            <fieldset class="nai-row" aria-label="增强倍数">
              {scales.map((scale) => (
                <button
                  key={scale}
                  type="button"
                  class="nai-chipbtn"
                  aria-pressed={enhanceScale === scale}
                  onClick={() => setEnhanceScale(scale)}
                >
                  {scale}×
                </button>
              ))}
            </fieldset>
            <label class="nai-label">
              变化力度
              <input
                class="nai-input"
                type="number"
                min={1}
                max={5}
                value={enhanceMag}
                onInput={(e) => setEnhanceMag(Number(e.currentTarget.value))}
              />
            </label>
            <button type="button" class="nai-btn nai-btn--primary" onClick={() => void enhance()}>
              开始增强
            </button>
          </div>
        )}
      </section>
      <aside class="nai-history" hidden={!props.historyOpen || props.hideHistory} aria-label="本次画廊">
        <h2>
          历史
          <PlayIcon size={12} />
        </h2>
        {props.shots.map((s, i) => (
          <button
            key={`${s.attrs.seed}-${i}`}
            type="button"
            class="wb-thumb"
            onClick={() => props.openShot(s)}
          >
            <img src={s.url} alt={`画廊图 ${s.attrs.seed}`} loading="lazy" decoding="async" />
          </button>
        ))}
      </aside>
    </section>
  );
}

const STAR = "M12 0 L14.4 9.2 L24 12 L14.4 14.8 L12 24 L9.6 14.8 L0 12 L9.6 9.2 Z";

function QuickstartGallery(props: { onPick: (prompt: string) => void }) {
  const cols: Array<Array<(typeof QUICKSTART)[number]>> = [[], [], []];
  for (let i = 0; i < QUICKSTART.length; i += 1) {
    const card = QUICKSTART[i];
    const col = cols[i % 3];
    if (card && col) col.push(card);
  }
  return (
    <div class="nv-quick">
      <header class="nv-quick__head">
        <svg class="nv-quick__spark" viewBox="0 0 520 150" aria-hidden="true">
          <path d={STAR} fill="currentColor" transform="translate(70 78) scale(1.35)" opacity="0.2" />
          <path d={STAR} fill="currentColor" transform="translate(150 28) scale(0.7)" opacity="0.55" />
          <path d={STAR} fill="currentColor" transform="translate(250 8) scale(1.6)" opacity="0.85" />
          <path d={STAR} fill="currentColor" transform="translate(360 18) scale(1.15)" opacity="0.7" />
          <path d={STAR} fill="currentColor" transform="translate(430 70) scale(0.85)" opacity="0.35" />
        </svg>
        <h2>开始创作</h2>
        <p>从图库里找一点灵感。</p>
        <p class="nv-quick__gap">点一张图，把提示词复制过来。</p>
      </header>
      <div class="nv-quick__cols">
        {cols.map((col) => (
          <div class="nv-quick__col" key={col[0]?.id ?? "col"}>
            {col.map((card) => (
              <button
                key={card.id}
                type="button"
                class="nv-quick__card"
                aria-label="复制这张图的提示词"
                onClick={() => props.onPick(card.prompt)}
              >
                <img src={quickstartSrc(card.id)} alt="" loading="lazy" decoding="async" />
              </button>
            ))}
          </div>
        ))}
      </div>
    </div>
  );
}

function SamplerPick(props: {
  model: string;
  value: string;
  open: boolean;
  compact?: boolean;
  onToggle: () => void;
  onPick: (id: string) => void;
}) {
  const all = samplersFor(props.model);
  const others = all.filter((s) => s !== props.value);
  return (
    <label title="采样器。决定去噪的走法，会改变画面气质。">
      采样器
      <div class="nv-pop">
        <button
          type="button"
          class={props.compact ? "nv-sampler" : "nv-sampler nv-sampler--wide"}
          aria-haspopup="listbox"
          aria-expanded={props.open}
          aria-label="采样器"
          onClick={(e) => {
            e.stopPropagation();
            props.onToggle();
          }}
        >
          <span class="nv-sampler__name">{SAMPLER_LABEL[props.value] ?? props.value}</span>
          <ChevronIcon size={14} />
        </button>
        {props.open && (
          <div class="nv-pop__menu nv-pop__menu--up" role="listbox" aria-label="采样器">
            <button type="button" role="option" aria-selected onClick={() => props.onPick(props.value)}>
              {SAMPLER_LABEL[props.value] ?? props.value}
            </button>
            {others.length > 0 && <p class="nv-model__group">其他</p>}
            {others.map((s) => (
              <button key={s} type="button" role="option" onClick={() => props.onPick(s)}>
                {SAMPLER_LABEL[s] ?? s}
              </button>
            ))}
          </div>
        )}
      </div>
    </label>
  );
}

type TokenCountState = { status: "loading" | "ready" | "error"; count: number };

function useModelTokenCount(model: string, text: string): TokenCountState {
  const [result, setResult] = useState<TokenCountState>({ status: "loading", count: 0 });
  useEffect(() => {
    let current = true;
    if (!text) {
      setResult({ status: "ready", count: 0 });
      return () => {
        current = false;
      };
    }
    setResult({ status: "loading", count: 0 });
    tokenizeText(model, text)
      .then((result) => {
        if (current) setResult({ status: "ready", count: result.ids.length });
      })
      .catch(() => {
        if (current) setResult({ status: "error", count: 0 });
      });
    return () => {
      current = false;
    };
  }, [model, text]);
  return result;
}

function TokenLengthBar({ model, text, limit }: { model: string; text: string; limit: number }) {
  const result = useModelTokenCount(model, text);
  const title =
    result.status === "ready"
      ? `${result.count} tokens / ${limit}`
      : result.status === "loading"
        ? "正在读取分词器…"
        : "分词器暂不可用";
  return (
    <div class="nv-bar" aria-hidden="true" title={title}>
      <span
        style={{
          width: `${result.status === "ready" ? Math.min(100, Math.round((result.count / limit) * 100)) : 0}%`,
        }}
      />
    </div>
  );
}

function appendTag(current: string, tag: string): string {
  const cur = current.trim();
  const escaped = tag.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  if (new RegExp(`(^|,\\s*)${escaped}(\\s*,|$)`).test(cur)) return cur;
  return cur ? `${cur}, ${tag}` : tag;
}
