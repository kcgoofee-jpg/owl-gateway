/**
 * NovelAI 风格生图页（原创实现）。
 * 布局规格（三栏 400/自适应/140、顶栏 52、面板配色与圆角）按参考站渲染结果测量重建，
 * 组件与样式全部为本仓库原创代码；数据层复用工作台既有网关客户端（api.ts / nai-config.ts）。
 * 暂未实现的能力（角色提示词、参考图、局部重绘、放大等）点击时给出明确提示。
 */

import { ArrowLeft, Download, Moon, Pencil, Settings, Sun, Wand2 } from "lucide-preact";
import { useEffect, useRef, useState } from "preact/hooks";
import {
  BASE_MODELS,
  buildRequest,
  clampForm,
  entryProblem,
  type GenAttrs,
  isV5,
  modelOf,
  type PaintForm,
  resolveSeed,
  SIZE_PRESETS,
  samplersFor,
  V5_MODELS,
} from "../../../lib/nai-config";
import { toast } from "../../../ui";
import { fetchSuggestions, makeShot, postImages, type Shot } from "../api";
import {
  AnlasIcon,
  CheckIcon,
  ChevronIcon,
  CloseIcon,
  CopyIcon,
  DiceIcon,
  HistoryIcon,
  LandscapeIcon,
  OwlIcon,
  PlusIcon,
  PortraitIcon,
  SeedIcon,
  SquareIcon,
  SwapIcon,
  TrashIcon,
} from "../Icons";
import { loadPrefs, savePrefs, type WbTheme } from "../prefs";
import { QUICKSTART } from "../quickstart";
import { me as meSignal, reloadMe, start } from "../store";
import {
  composeNegative,
  composePositive,
  type QualityPreset,
  qualityPresetLabel,
  qualityPresetList,
  UC_LABELS,
  type UcPreset,
  ucPresetList,
} from "../workbench-config";
import "./nai.css";

const SAMPLER_LABELS: Record<string, string> = {
  k_euler_ancestral: "Euler Ancestral",
  k_euler: "Euler",
  k_dpmpp_2s_ancestral: "DPM++ 2S Ancestral",
  k_dpmpp_sde: "DPM++ SDE",
  k_dpmpp_2m: "DPM++ 2M",
  k_lms: "LMS",
  k_dpm_2: "DPM 2",
  k_dpm_2_ancestral: "DPM 2 Ancestral",
  ddim: "DDIM",
};

const samplerLabel = (id: string): string => SAMPLER_LABELS[id] ?? id;

/** 快速开始卡片的原创装饰底：纯 CSS 渐变，按序号取一组色调。 */
const QS_ART: Array<[string, string, string]> = [
  ["#2c1f4a", "#5b3f8f", "#f5c66d"],
  ["#12303f", "#2e6f7e", "#9fe8d8"],
  ["#3a1626", "#8f2f52", "#ffb7c5"],
  ["#1d2a18", "#4a6b3a", "#d6e8a0"],
  ["#241733", "#6b3fa0", "#e0c3ff"],
  ["#0f2536", "#3a6ea5", "#a8d8ff"],
  ["#362417", "#a56b3f", "#ffd9a8"],
  ["#2b1230", "#7a3f8f", "#f0b7ff"],
  ["#142e2a", "#3f7a6b", "#b7ffe8"],
  ["#301717", "#8f3f3f", "#ffb7b7"],
  ["#1a1f36", "#3f4f8f", "#b7c9ff"],
  ["#2e2a14", "#8f7a3f", "#fff0b7"],
];

const qsArt = (i: number): string => {
  const [a, b, c] = QS_ART[i % QS_ART.length] ?? QS_ART[0] ?? ["#222", "#555", "#aaa"];
  return (
    `radial-gradient(120% 90% at 78% 12%, ${c}33 0%, transparent 52%),` +
    `radial-gradient(90% 70% at 18% 82%, ${b}55 0%, transparent 58%),` +
    `linear-gradient(158deg, ${a} 20%, ${b}cc 100%)`
  );
};

/* ---------------- 小组件 ---------------- */

/** 点击外部关闭的下拉容器。 */
function Pop(props: {
  open: boolean;
  onClose: () => void;
  align?: "left" | "right";
  children: preact.ComponentChildren;
}) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!props.open) return;
    const onDown = (e: Event) => {
      if (ref.current && !ref.current.contains(e.target as Node)) props.onClose();
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") props.onClose();
    };
    document.addEventListener("mousedown", onDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [props.open, props.onClose]);
  if (!props.open) return null;
  return (
    <div ref={ref} class={`nai-pop-panel${props.align === "right" ? " nai-pop-panel--right" : ""}`}>
      {props.children}
    </div>
  );
}

function PopItem(props: {
  selected?: boolean;
  disabled?: boolean;
  onSelect: () => void;
  children: preact.ComponentChildren;
  sub?: string | undefined;
}) {
  return (
    <button
      type="button"
      class="nai-pop-item"
      data-selected={props.selected ? "true" : "false"}
      disabled={props.disabled}
      onClick={props.onSelect}
    >
      <span>{props.children}</span>
      {props.sub ? <span class="nai-pop-item-sub">{props.sub}</span> : null}
    </button>
  );
}

/* ---------------- 主应用 ---------------- */

export function NaiApp() {
  useEffect(() => start(), []);
  const me = meSignal.value;

  /** 游客 / 成员收窄：游客也能看完整界面，出图时才拦。 */
  const member = me?.logged_in === true ? me : undefined;
  const paid = member?.policy?.paid_parameters !== false;
  const v5Open = member?.policy?.v5 === true;

  const [theme, setTheme] = useState<WbTheme>(() => loadPrefs().theme);
  const [model, setModel] = useState<string>(() =>
    v5Open ? "nai-diffusion-5-curated" : "nai-diffusion-4-5-full",
  );
  const [mode, setMode] = useState<"anime" | "text">("anime");
  const [prompt, setPrompt] = useState("");
  const [negative, setNegative] = useState("");
  const [qualityPreset, setQualityPreset] = useState<QualityPreset>("standard");
  const [ucPreset, setUcPreset] = useState<UcPreset>("heavy");
  const [transparent, setTransparent] = useState(false);
  const [ucOpen, setUcOpen] = useState(true);
  const [width, setWidth] = useState(832);
  const [height, setHeight] = useState(1216);
  const [presetId, setPresetId] = useState("portrait");
  const [count, setCount] = useState(1);
  const [steps, setSteps] = useState(23);
  const [scale, setScale] = useState(v5Open ? 7 : 5);
  const [sampler, setSampler] = useState("k_euler_ancestral");
  const [seed, setSeed] = useState("-1");
  const [shots, setShots] = useState<Shot[]>([]);
  const [viewing, setViewing] = useState<number | null>(null);
  const [busy, setBusy] = useState(false);
  const [notes, setNotes] = useState<string[]>([]);
  const [welcome, setWelcome] = useState(() => {
    try {
      return localStorage.getItem("owl-nai-welcome-v1") !== "1";
    } catch {
      return true;
    }
  });
  const [pop, setPop] = useState<"" | "model" | "mode" | "quality" | "uc" | "size" | "sampler">("");
  const [suggest, setSuggest] = useState<{ tags: string[] }>({ tags: [] });
  const [historyOpen, setHistoryOpen] = useState(false);

  const abortRef = useRef<AbortController | null>(null);
  useEffect(() => () => abortRef.current?.abort(), []);

  const closeWelcome = () => {
    setWelcome(false);
    try {
      localStorage.setItem("owl-nai-welcome-v1", "1");
    } catch {
      /* 隐私模式写不进去就每次都弹 */
    }
  };

  const pickModel = (id: string) => {
    setModel(id);
    setPop("");
    setScale(isV5(id) ? 7 : 5);
    if (!samplersFor(id).includes(sampler)) setSampler(samplersFor(id)[0] ?? "k_euler");
  };

  /* 尺寸分类：与预设联动 */
  const applyPreset = (id: string) => {
    const p = SIZE_PRESETS.find((s) => s.id === id);
    setPresetId(id);
    if (p?.w && p?.h) {
      setWidth(p.w);
      setHeight(p.h);
    }
  };
  const orientOf = (): "portrait" | "landscape" | "square" =>
    width === height ? "square" : width > height ? "landscape" : "portrait";
  const applyOrient = (o: "portrait" | "landscape" | "square") => {
    if (o === "square") {
      const s = Math.max(width, height);
      setWidth(s);
      setHeight(s);
    } else if (o === "landscape" && width < height) {
      setWidth(height);
      setHeight(width);
    } else if (o === "portrait" && width > height) {
      setWidth(height);
      setHeight(width);
    }
    setPresetId("custom");
  };

  /* 标签建议：有权限且输入非空时防抖拉取 */
  useEffect(() => {
    const key = member?.key;
    if (!key || member?.policy?.tags === false) return;
    const text = prompt.trim();
    if (text.length < 2) {
      setSuggest({ tags: [] });
      return;
    }
    const ctrl = new AbortController();
    const timer = window.setTimeout(() => {
      fetchSuggestions(text, model, key, ctrl.signal)
        .then((tags) => setSuggest({ tags }))
        .catch(() => setSuggest({ tags: [] }));
    }, 600);
    return () => {
      window.clearTimeout(timer);
      ctrl.abort();
    };
  }, [prompt, model, member?.key, member?.policy?.tags === false]);

  const composeForm = (): PaintForm => ({
    positive: composePositive(model, prompt, "", qualityPreset),
    negative: composeNegative(model, negative, ucPreset, composePositive(model, prompt, "", qualityPreset)),
    model,
    preset: presetId,
    width,
    height,
    steps,
    scale,
    sampler,
    noise: "karras",
    cfgRescale: 0,
    seed,
    smea: false,
  });

  const generate = async () => {
    if (!member) {
      toast("先用 Discord 登录，拿到 Key 之后就能出图。", { error: true });
      return;
    }
    const entry = entryProblem(member);
    if (entry) {
      toast(entry, { error: true });
      return;
    }
    const form = composeForm();
    /* 校验用户原始输入：composePositive 会拼上质量词，composed 结果永远非空，不能用它判空。 */
    if (!prompt.trim()) {
      toast("先写点提示词吧，比如：1girl, sunset, very aesthetic");
      return;
    }
    const clamped = clampForm(form, paid);
    setNotes(clamped.notes);
    const key = member.key ?? "";
    const ctrl = new AbortController();
    abortRef.current = ctrl;
    setBusy(true);
    const made: Shot[] = [];
    const baseIndex = shots.length;
    try {
      for (let i = 0; i < count; i++) {
        const used = count > 1 ? (resolveSeed(seed) + i) >>> 0 : resolveSeed(seed);
        const req = buildRequest({ ...clamped.form, seed: String(used) }, used);
        if (transparent) (req.parameters as Record<string, unknown>).transparent_background = true;
        const startedAt = Date.now();
        const imgs = await postImages(req, key, ctrl.signal);
        const attrs: GenAttrs = {
          model,
          width: clamped.form.width,
          height: clamped.form.height,
          steps: clamped.form.steps,
          scale: clamped.form.scale,
          sampler: clamped.form.sampler,
          noise: clamped.form.noise,
          seed: used,
          prompt: clamped.form.positive,
          negative: clamped.form.negative,
          seconds: Math.round((Date.now() - startedAt) / 100) / 10,
        };
        for (const img of imgs) made.push(makeShot(img, attrs, count > 1 ? `${i + 1}/${count}` : undefined));
      }
      setShots((prev) => [...prev, ...made]);
      setViewing(baseIndex);
      toast(count > 1 ? `完成了 ${made.length} 张。` : "完成了。");
      void reloadMe();
    } catch (e) {
      if (e instanceof DOMException && e.name === "AbortError") return;
      toast(e instanceof Error ? e.message : "出图没有成功，稍后再试一次。", { error: true });
    } finally {
      setBusy(false);
      abortRef.current = null;
    }
  };

  const shot = viewing !== null ? shots[viewing] : undefined;
  const anlas = member?.today?.anlas ?? 0;

  const swapSize = () => {
    setWidth(height);
    setHeight(width);
    setPresetId("custom");
  };

  return (
    <div class="nai-root" data-theme={theme}>
      {/* 顶栏 */}
      <header class="nai-topbar">
        <div class="nai-topbar-inner">
          <span>猫头鹰公益站 · 生图工作台</span>
          <div class="nai-topbar-actions">
            <a class="nai-topbar-signup" href="/login">
              Sign Up
            </a>
            <a class="nai-topbar-login" href="/login">
              Log In
            </a>
          </div>
        </div>
      </header>

      <div class="nai-body">
        {/* 左侧设置栏 */}
        <aside class="nai-sidebar">
          <div class="nai-sidebar-head">
            <span class="nai-brand">
              <OwlIcon size={26} />
              猫头鹰公益站
            </span>
            <div class="nai-head-actions">
              <button
                type="button"
                class="nai-icon-btn"
                title="切换主题"
                aria-label="切换主题"
                onClick={() => {
                  const order: WbTheme[] = ["dark", "sand", "slate"];
                  const next = order[(order.indexOf(theme) + 1) % order.length] ?? "dark";
                  setTheme(next);
                  savePrefs({ ...loadPrefs(), theme: next });
                }}
              >
                {theme === "dark" ? (
                  <Moon size={17} />
                ) : theme === "sand" ? (
                  <Sun size={17} />
                ) : (
                  <Moon size={17} />
                )}
              </button>
              <button
                type="button"
                class="nai-icon-btn"
                title="设置"
                aria-label="设置"
                onClick={() => toast("主题切换在旁边的小图标；其余设置沿用旧版工作台。")}
              >
                <Settings size={17} />
              </button>
            </div>
          </div>

          <div class="nai-sidebar-scroll">
            <div class="nai-groups">
              {/* 模型 / 模式 */}
              <div class="nai-pills-row">
                <div class="nai-pop" style="flex:1.2">
                  <button
                    type="button"
                    class="nai-pill"
                    style="width:100%"
                    onClick={() => setPop(pop === "model" ? "" : "model")}
                  >
                    <span class="nai-pill-label">Model</span>
                    <span class="nai-pill-value">
                      <span>{modelOf(model).label}</span>
                      <ChevronIcon size={14} open={pop === "model"} />
                    </span>
                  </button>
                  <Pop open={pop === "model"} onClose={() => setPop("")}>
                    <div class="nai-pop-group">新模型{v5Open ? "" : "（需要站长为你的 Key 开放）"}</div>
                    {V5_MODELS.map((m) => (
                      <PopItem
                        key={m.id}
                        selected={model === m.id}
                        disabled={!v5Open}
                        sub={v5Open ? undefined : "未开放"}
                        onSelect={() => pickModel(m.id)}
                      >
                        {m.label}
                      </PopItem>
                    ))}
                    <div class="nai-pop-group">常规模型</div>
                    {BASE_MODELS.map((m) => (
                      <PopItem key={m.id} selected={model === m.id} onSelect={() => pickModel(m.id)}>
                        {m.label}
                      </PopItem>
                    ))}
                  </Pop>
                </div>
                <div class="nai-pop" style="flex:1">
                  <button
                    type="button"
                    class="nai-pill"
                    style="width:100%"
                    onClick={() => setPop(pop === "mode" ? "" : "mode")}
                  >
                    <span class="nai-pill-label">Mode</span>
                    <span class="nai-pill-value">
                      <span>{mode === "anime" ? "Anime" : "Text"}</span>
                      <ChevronIcon size={14} open={pop === "mode"} />
                    </span>
                  </button>
                  <Pop open={pop === "mode"} onClose={() => setPop("")}>
                    <PopItem
                      selected={mode === "anime"}
                      onSelect={() => {
                        setMode("anime");
                        setPop("");
                      }}
                    >
                      Anime
                    </PopItem>
                    <PopItem
                      selected={mode === "text"}
                      onSelect={() => {
                        setMode("text");
                        setPop("");
                        toast("Text 模式只影响官方站的标签建议，本站统一按动漫标签处理。");
                      }}
                    >
                      Text
                    </PopItem>
                  </Pop>
                </div>
              </div>

              {/* 正面提示词 */}
              <div class="nai-block">
                <div class="nai-block-head">
                  <span class="nai-block-title">Prompt</span>
                  <div class="nai-block-actions">
                    <button
                      type="button"
                      class="nai-icon-btn"
                      title="标签补全"
                      aria-label="标签补全"
                      onClick={() => {
                        const t = suggest.tags[0];
                        if (!t) toast("先写几个字，再等一拍就会有标签建议。");
                        else setPrompt((p) => (p ? `${p.replace(/,\s*$/, "")}, ${t}` : t));
                      }}
                    >
                      <PlusIcon size={16} />
                    </button>
                    <button
                      type="button"
                      class="nai-icon-btn"
                      title="清空提示词"
                      aria-label="清空提示词"
                      onClick={() => setPrompt("")}
                    >
                      <TrashIcon size={15} />
                    </button>
                  </div>
                </div>
                <div class="nai-block-body">
                  <textarea
                    class="nai-textarea"
                    placeholder="写下你想画的内容，比如：1girl, sunset, very aesthetic"
                    value={prompt}
                    onInput={(e) => setPrompt((e.target as HTMLTextAreaElement).value)}
                  />
                </div>
                <div class="nai-block-foot">
                  <div class="nai-block-foot-row">
                    <span class="nai-quality-note">
                      Quality Tags: <b>{qualityPresetLabel(qualityPreset)}</b>
                    </span>
                    <div class="nai-pop">
                      <button
                        type="button"
                        class="nai-miniselect"
                        onClick={() => setPop(pop === "quality" ? "" : "quality")}
                      >
                        <span>Quality Preset</span>
                        <ChevronIcon size={12} />
                      </button>
                      <Pop open={pop === "quality"} onClose={() => setPop("")} align="right">
                        {qualityPresetList(model).map((q) => (
                          <PopItem
                            key={q}
                            selected={qualityPreset === q}
                            onSelect={() => {
                              setQualityPreset(q);
                              setPop("");
                            }}
                          >
                            {qualityPresetLabel(q)}
                          </PopItem>
                        ))}
                      </Pop>
                    </div>
                  </div>
                  <div class="nai-chips-row">
                    <button
                      type="button"
                      class="nai-chip"
                      aria-pressed={transparent ? "true" : "false"}
                      title="透明背景"
                      onClick={() => {
                        if (model.includes("diffusion-3")) toast("这个模型不支持透明背景输出。");
                        else setTransparent(!transparent);
                      }}
                    >
                      Transparent BG
                    </button>
                    {suggest.tags.slice(0, 4).map((t) => (
                      <button
                        key={t}
                        type="button"
                        class="nai-chip"
                        title="点击追加到提示词"
                        onClick={() => setPrompt((p) => (p ? `${p.replace(/,\s*$/, "")}, ${t}` : t))}
                      >
                        {t}
                      </button>
                    ))}
                  </div>
                </div>
              </div>

              {/* 负面提示词 */}
              <div class={`nai-block nai-block--uc${ucOpen ? "" : " nai-block--collapsed"}`}>
                <div class="nai-block-head">
                  <span class="nai-block-title">Undesired Content</span>
                  <button
                    type="button"
                    class="nai-icon-btn"
                    title={ucOpen ? "收起" : "展开"}
                    aria-label={ucOpen ? "收起负面提示词" : "展开负面提示词"}
                    onClick={() => setUcOpen(!ucOpen)}
                  >
                    <ChevronIcon size={14} open={ucOpen} />
                  </button>
                </div>
                <div class="nai-block-body">
                  <textarea
                    class="nai-textarea"
                    placeholder="不希望出现的内容（低质量词由预设自动带上）"
                    value={negative}
                    onInput={(e) => setNegative((e.target as HTMLTextAreaElement).value)}
                  />
                </div>
                <div class="nai-block-foot">
                  <div class="nai-block-foot-row">
                    <span class="nai-quality-note">
                      UC Preset: <b>{UC_LABELS[ucPreset]}</b>
                    </span>
                    <div class="nai-pop">
                      <button
                        type="button"
                        class="nai-miniselect"
                        onClick={() => setPop(pop === "uc" ? "" : "uc")}
                      >
                        <span>Undesired Content Preset</span>
                        <ChevronIcon size={12} />
                      </button>
                      <Pop open={pop === "uc"} onClose={() => setPop("")} align="right">
                        {ucPresetList(model).map((u) => (
                          <PopItem
                            key={u}
                            selected={ucPreset === u}
                            onSelect={() => {
                              setUcPreset(u);
                              setPop("");
                            }}
                          >
                            {UC_LABELS[u]}
                          </PopItem>
                        ))}
                      </Pop>
                    </div>
                  </div>
                </div>
              </div>

              {/* 角色提示词（暂未开放） */}
              <div class="nai-card">
                <div class="nai-card-row">
                  <span class="nai-card-text">为场景中的角色分别写提示词，本站暂未开放。</span>
                  <button
                    type="button"
                    class="nai-card-expand"
                    title="角色提示词"
                    aria-label="打开角色提示词"
                    onClick={() => toast("角色提示词暂未开放，多角色请直接写在提示词里。")}
                  >
                    <PlusIcon size={16} />
                  </button>
                </div>
              </div>

              {/* 参考图（暂未开放） */}
              <div class="nai-settings" style="gap:8px">
                <span class="nai-group-label">Reference Images</span>
                <div class="nai-i2i-box">
                  <div>
                    <div class="nai-i2i-title">Image2Image</div>
                    <div class="nai-i2i-sub">以参考图为基础变换画面，本站暂未开放。</div>
                  </div>
                  <button
                    type="button"
                    class="nai-card-expand"
                    title="图生图"
                    aria-label="打开图生图"
                    onClick={() => toast("图生图暂未开放，需要时可用旧版工作台（?legacy=1）。")}
                  >
                    <ChevronIcon size={14} />
                  </button>
                </div>
              </div>

              {/* 图像设置 */}
              <div class="nai-settings">
                <span class="nai-group-label">Image Settings</span>

                <div style="display:flex;flex-direction:column;gap:8px">
                  <span class="nai-set-label">Resolution</span>
                  <div class="nai-set-row" style="justify-content:flex-start">
                    <div class="nai-number">
                      <input
                        type="number"
                        min={64}
                        max={3072}
                        step={64}
                        aria-label="宽"
                        value={width}
                        onInput={(e) => {
                          setWidth(Number((e.target as HTMLInputElement).value) || 64);
                          setPresetId("custom");
                        }}
                      />
                      <button
                        type="button"
                        class="nai-number-swap"
                        title="交换宽高"
                        aria-label="交换宽高"
                        onClick={swapSize}
                      >
                        <SwapIcon size={12} />
                      </button>
                      <input
                        type="number"
                        min={64}
                        max={3072}
                        step={64}
                        aria-label="高"
                        value={height}
                        onInput={(e) => {
                          setHeight(Number((e.target as HTMLInputElement).value) || 64);
                          setPresetId("custom");
                        }}
                      />
                    </div>
                  </div>
                  <div class="nai-size-row">
                    <div class="nai-pop" style="flex:1">
                      <button
                        type="button"
                        class="nai-size-select"
                        onClick={() => setPop(pop === "size" ? "" : "size")}
                      >
                        <span>{SIZE_PRESETS.find((s) => s.id === presetId)?.label ?? "自定义"}</span>
                        <ChevronIcon size={12} open={pop === "size"} />
                      </button>
                      <Pop open={pop === "size"} onClose={() => setPop("")}>
                        {SIZE_PRESETS.map((s) => (
                          <PopItem
                            key={s.id}
                            selected={presetId === s.id}
                            onSelect={() => {
                              applyPreset(s.id);
                              setPop("");
                            }}
                          >
                            {s.label}
                          </PopItem>
                        ))}
                      </Pop>
                    </div>
                    <button
                      type="button"
                      class="nai-orient-btn"
                      title="竖图"
                      aria-label="竖图"
                      aria-pressed={orientOf() === "portrait" ? "true" : "false"}
                      onClick={() => applyOrient("portrait")}
                    >
                      <PortraitIcon size={15} />
                    </button>
                    <button
                      type="button"
                      class="nai-orient-btn"
                      title="横图"
                      aria-label="横图"
                      aria-pressed={orientOf() === "landscape" ? "true" : "false"}
                      onClick={() => applyOrient("landscape")}
                    >
                      <LandscapeIcon size={15} />
                    </button>
                    <button
                      type="button"
                      class="nai-orient-btn"
                      title="方图"
                      aria-label="方图"
                      aria-pressed={orientOf() === "square" ? "true" : "false"}
                      onClick={() => applyOrient("square")}
                    >
                      <SquareIcon size={15} />
                    </button>
                  </div>
                </div>

                <div>
                  <div class="nai-set-label" style="margin-bottom:6px">
                    Number of Images
                  </div>
                  <div class="nai-segment">
                    {[1, 2, 3, 4].map((n) => (
                      <button
                        key={n}
                        type="button"
                        aria-pressed={count === n ? "true" : "false"}
                        onClick={() => setCount(n)}
                      >
                        {n}
                      </button>
                    ))}
                  </div>
                </div>

                <div class="nai-set-row">
                  <span class="nai-set-label">Steps</span>
                  <div class="nai-number">
                    <input
                      type="number"
                      min={1}
                      max={50}
                      aria-label="步数"
                      value={steps}
                      onInput={(e) =>
                        setSteps(Math.max(1, Math.min(50, Number((e.target as HTMLInputElement).value) || 1)))
                      }
                    />
                  </div>
                </div>

                <div class="nai-set-row">
                  <span class="nai-set-label">Guidance</span>
                  <div class="nai-number">
                    <input
                      type="number"
                      min={0}
                      max={10}
                      step={0.5}
                      aria-label="引导系数"
                      value={scale}
                      onInput={(e) =>
                        setScale(Math.max(0, Math.min(10, Number((e.target as HTMLInputElement).value) || 0)))
                      }
                    />
                  </div>
                </div>

                <div>
                  <div class="nai-set-label" style="margin-bottom:6px">
                    Seed
                  </div>
                  <div class="nai-seed-row">
                    <div class="nai-seed-input">
                      <input
                        type="text"
                        inputmode="numeric"
                        placeholder="-1 为随机"
                        aria-label="种子"
                        value={seed}
                        onInput={(e) => setSeed((e.target as HTMLInputElement).value)}
                      />
                    </div>
                    <button
                      type="button"
                      class="nai-square-btn"
                      title="随机种子"
                      aria-label="随机种子"
                      onClick={() => setSeed(String(Math.floor(Math.random() * 4294967296)))}
                    >
                      <DiceIcon size={15} />
                    </button>
                    <button
                      type="button"
                      class="nai-square-btn"
                      title="使用当前图片的种子"
                      aria-label="使用当前图片的种子"
                      disabled={!shot}
                      onClick={() => {
                        if (shot) setSeed(String(shot.attrs.seed));
                      }}
                    >
                      <SeedIcon size={15} />
                    </button>
                  </div>
                </div>

                <div>
                  <div class="nai-set-label" style="margin-bottom:6px">
                    Sampler
                  </div>
                  <div class="nai-pop">
                    <button
                      type="button"
                      class="nai-sampler-select"
                      onClick={() => setPop(pop === "sampler" ? "" : "sampler")}
                    >
                      <span>{samplerLabel(sampler)}</span>
                      <ChevronIcon size={14} open={pop === "sampler"} />
                    </button>
                    <Pop open={pop === "sampler"} onClose={() => setPop("")} align="right">
                      {samplersFor(model).map((s) => (
                        <PopItem
                          key={s}
                          selected={sampler === s}
                          onSelect={() => {
                            setSampler(s);
                            setPop("");
                          }}
                        >
                          {samplerLabel(s)}
                        </PopItem>
                      ))}
                    </Pop>
                  </div>
                </div>
              </div>
            </div>
          </div>

          <div class="nai-divider" />

          <div class="nai-sidebar-bottom">
            {notes.length > 0 && (
              <div class="nai-status" role="status">
                {notes.join("；")}
              </div>
            )}
            <button type="button" class="nai-generate" disabled={busy} onClick={() => void generate()}>
              <span>{busy ? "生成中…" : `Generate ${count} Image${count > 1 ? "s" : ""}`}</span>
              <span class="nai-generate-badge">
                {count}
                <AnlasIcon size={12} />
              </span>
            </button>
          </div>
        </aside>

        {/* 画布区 */}
        <main class="nai-canvas">
          {shot ? (
            <div class="nai-viewer">
              <div class="nai-viewer-bar">
                <button
                  type="button"
                  class="nai-icon-btn"
                  title="返回"
                  aria-label="返回画廊"
                  onClick={() => setViewing(null)}
                >
                  <ArrowLeft size={18} />
                </button>
                <span class="nai-anlas-chip">
                  Anlas: {anlas}
                  <AnlasIcon size={12} />
                </span>
                <div class="nai-viewer-bar-spacer" />
                <a
                  class="nai-icon-btn"
                  title="下载"
                  aria-label="下载图片"
                  href={shot.url}
                  download={`owl-${shot.attrs.seed}.${shot.ext}`}
                >
                  <Download size={17} />
                </a>
                <button
                  type="button"
                  class="nai-icon-btn"
                  title="用这颗种子再画"
                  aria-label="用这颗种子再画"
                  onClick={() => setSeed(String(shot.attrs.seed))}
                >
                  <SeedIcon size={16} />
                </button>
                <button
                  type="button"
                  class="nai-icon-btn"
                  title="放大（暂未开放）"
                  aria-label="放大"
                  onClick={() => toast("放大功能暂未开放，需要时可用旧版工作台（?legacy=1）。")}
                >
                  <Wand2 size={17} />
                </button>
                <button
                  type="button"
                  class="nai-icon-btn"
                  title="局部重绘（暂未开放）"
                  aria-label="局部重绘"
                  onClick={() => toast("局部重绘暂未开放，需要时可用旧版工作台（?legacy=1）。")}
                >
                  <Pencil size={17} />
                </button>
                <button
                  type="button"
                  class="nai-icon-btn"
                  title="从历史移除"
                  aria-label="从历史移除"
                  onClick={() => {
                    const idx = viewing ?? 0;
                    setShots((prev) => prev.filter((_, i) => i !== idx));
                    setViewing(null);
                  }}
                >
                  <TrashIcon size={15} />
                </button>
              </div>
              <div class="nai-viewer-stage">
                <img src={shot.url} alt={`生成结果 ${shot.attrs.seed}`} />
              </div>
              <div class="nai-viewer-meta">
                <span>{modelOf(shot.attrs.model).label}</span>
                <span>
                  {shot.attrs.width}×{shot.attrs.height}
                </span>
                <span>{shot.attrs.steps} Steps</span>
                <span>{samplerLabel(shot.attrs.sampler)}</span>
                <span>Seed {shot.attrs.seed}</span>
                {shot.attrs.seconds > 0 && <span>{shot.attrs.seconds}s</span>}
              </div>
            </div>
          ) : (
            <div class="nai-gallery">
              <div class="nai-gallery-hero">
                <svg viewBox="0 0 691 258" aria-hidden="true">
                  <defs>
                    <linearGradient id="nai-hero-g" x1="0" y1="0" x2="1" y2="1">
                      <stop offset="0" stop-color="var(--nai-accent)" stop-opacity="0.55" />
                      <stop offset="1" stop-color="var(--nai-accent)" stop-opacity="0.06" />
                    </linearGradient>
                  </defs>
                  <g fill="none" stroke="url(#nai-hero-g)" stroke-width="2">
                    <circle cx="140" cy="120" r="96" />
                    <circle cx="140" cy="120" r="64" />
                    <circle cx="140" cy="120" r="30" />
                    <path d="M300 200 C 380 120, 470 120, 560 200" />
                    <path d="M320 226 C 400 156, 480 156, 545 226" />
                    <path d="M345 60 L 360 88 L 388 60 L 373 32 Z" />
                    <path d="M520 40 L 531 60 L 552 40 L 541 20 Z" />
                  </g>
                  <g fill="var(--nai-accent)" opacity="0.5">
                    <circle cx="240" cy="60" r="3" />
                    <circle cx="620" cy="90" r="3" />
                    <circle cx="440" cy="34" r="3" />
                    <circle cx="580" cy="180" r="3" />
                  </g>
                </svg>
                <h1 class="nai-gallery-title">Get Started</h1>
              </div>
              <p class="nai-gallery-sub">从快速开始图库里找灵感！</p>
              <p class="nai-gallery-hint">点击卡片复制提示词。</p>
              <div class="nai-qs-grid">
                {QUICKSTART.slice(0, 12).map((card, i) => (
                  <QsCard key={card.id} index={i} prompt={card.prompt} />
                ))}
              </div>
            </div>
          )}
        </main>

        {/* 右侧工具栏 */}
        <aside class={`nai-rail${historyOpen ? " nai-rail--history" : ""}`}>
          <div class="nai-rail-head">
            <button
              type="button"
              class="nai-rail-history"
              aria-expanded={historyOpen ? "true" : "false"}
              onClick={() => setHistoryOpen(!historyOpen)}
            >
              <HistoryIcon size={18} />
              <span>History</span>
            </button>
          </div>
          <div class="nai-rail-tools">
            {historyOpen ? (
              shots.length ? (
                <div class="nai-history-grid">
                  {shots.map((s, i) => (
                    <button
                      key={`${s.attrs.seed}-${i}`}
                      type="button"
                      class="nai-history-item"
                      data-active={viewing === i ? "true" : "false"}
                      title={`${s.attrs.seed} · ${s.attrs.width}×${s.attrs.height}`}
                      onClick={() => {
                        setViewing(i);
                        setHistoryOpen(false);
                      }}
                    >
                      <img src={s.url} alt={`历史 ${i + 1}`} loading="lazy" />
                    </button>
                  ))}
                </div>
              ) : (
                <div class="nai-history-empty">还没有生成过图片。</div>
              )
            ) : (
              shot && (
                <>
                  <a
                    class="nai-tool-btn"
                    title="下载"
                    aria-label="下载图片"
                    href={shot.url}
                    download={`owl-${shot.attrs.seed}.${shot.ext}`}
                  >
                    <Download size={20} />
                  </a>
                  <button
                    type="button"
                    class="nai-tool-btn"
                    title="用这颗种子再画"
                    aria-label="用这颗种子再画"
                    onClick={() => setSeed(String(shot.attrs.seed))}
                  >
                    <SeedIcon size={20} />
                  </button>
                  <button
                    type="button"
                    class="nai-tool-btn"
                    title="放大（暂未开放）"
                    aria-label="放大"
                    onClick={() => toast("放大功能暂未开放，需要时可用旧版工作台（?legacy=1）。")}
                  >
                    <Wand2 size={20} />
                  </button>
                  <button
                    type="button"
                    class="nai-tool-btn"
                    title="局部重绘（暂未开放）"
                    aria-label="局部重绘"
                    onClick={() => toast("局部重绘暂未开放，需要时可用旧版工作台（?legacy=1）。")}
                  >
                    <Pencil size={20} />
                  </button>
                </>
              )
            )}
          </div>
          <div class="nai-rail-foot">
            <button
              type="button"
              class="nai-icon-btn"
              title="切换主题"
              aria-label="切换主题"
              onClick={() => {
                const order: WbTheme[] = ["dark", "sand", "slate"];
                const next = order[(order.indexOf(theme) + 1) % order.length] ?? "dark";
                setTheme(next);
                savePrefs({ ...loadPrefs(), theme: next });
              }}
            >
              {theme === "sand" ? <Sun size={17} /> : <Moon size={17} />}
            </button>
            <button
              type="button"
              class="nai-icon-btn"
              title="回到旧版工作台"
              aria-label="回到旧版工作台"
              onClick={() => {
                location.href = "/workbench?legacy=1";
              }}
            >
              <Settings size={17} />
            </button>
          </div>
        </aside>
      </div>

      {/* 欢迎弹窗 */}
      {welcome && (
        <div class="nai-overlay" role="dialog" aria-modal="true" aria-label="欢迎">
          <div class="nai-modal">
            <button type="button" class="nai-modal-close" aria-label="关闭" onClick={closeWelcome}>
              <CloseIcon size={16} />
            </button>
            <div class="nai-modal-body">
              <div class="nai-modal-mark">
                <OwlIcon size={32} />
              </div>
              <div class="nai-modal-title">欢迎来到猫头鹰公益站</div>
              <div class="nai-modal-sub">AI 动漫图像生成 · 网页直接出图</div>
              <ul class="nai-modal-list">
                <li>每日免费出图 {member?.today?.daily_images ?? member?.policy?.daily_images ?? "—"} 张</li>
                <li>
                  V5 模型每日 {member?.today?.daily_v5 ?? member?.policy?.daily_v5 ?? "—"} 张（按 Key 开放）
                </li>
                <li>参数与官方客户端一致，出图即记账</li>
              </ul>
              <button type="button" class="nai-modal-cta" onClick={closeWelcome}>
                开始创作
              </button>
              <span class="nai-modal-legacy">
                习惯旧版界面？<a href="/workbench?legacy=1">点这里切换</a>
              </span>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

/* ---------------- 快速开始卡片 ---------------- */

function QsCard(props: { index: number; prompt: string }) {
  const [copied, setCopied] = useState(false);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(props.prompt);
      setCopied(true);
      toast("提示词已复制。");
      window.setTimeout(() => setCopied(false), 1500);
    } catch {
      toast("复制失败，浏览器不允许剪贴板写入。", { error: true });
    }
  };
  return (
    <button type="button" class="nai-qs-card" title="点击复制提示词" onClick={() => void copy()}>
      <span class="nai-qs-art" style={{ background: qsArt(props.index) }} aria-hidden="true" />
      <span class="nai-qs-copy">
        {copied ? <CheckIcon size={16} /> : <CopyIcon size={16} />}
        {copied ? "已复制" : "Copied!"}
      </span>
    </button>
  );
}
