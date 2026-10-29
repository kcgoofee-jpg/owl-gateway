/**
 * NovelAI 风格生图页（原创实现）。
 * 布局规格（导航行 62、侧栏 400、画布自适应、右轨 140、面板配色与圆角）按参考站渲染结果
 * 测量重建（见 docs/nai-ui-catalog.md），组件与样式全部为本仓库原创代码；
 * 数据层复用工作台既有网关客户端（api.ts / nai-config.ts / tokenizer.ts）。
 * 暂未实现的能力（角色提示词、图生图、放大、变体、局部重绘等）点击时给出明确提示。
 */

import { ArrowRight, Clock, Download, LayoutGrid, LogIn, Pencil, Settings, User, Wand2 } from "lucide-preact";
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
  CloseIcon,
  CopyIcon,
  ExpandIcon,
  LandscapeIcon,
  MenuIcon,
  PinIcon,
  PlayIcon,
  PlusIcon,
  PortraitIcon,
  SeedIcon,
  SparkleIcon,
  SquareIcon,
  SwapIcon,
  TrashIcon,
  UploadIcon,
} from "../Icons";
import { loadPrefs, savePrefs, type WbTheme } from "../prefs";
import { QUICKSTART } from "../quickstart";
import { me as meSignal, reloadMe, start } from "../store";
import { tokenizeText } from "../tokenizer";
import {
  composeNegative,
  composePositive,
  MODEL_BLURBS,
  type QualityPreset,
  qualityPresetList,
  type UcPreset,
  ucPresetList,
} from "../workbench-config";
import "./nai.css";

const SAMPLER_LABELS: Record<string, string> = {
  k_euler_ancestral: "Euler Ancestral",
  k_euler: "Euler",
  k_dpmpp_2s_ancestral: "DPM++ 2S Ancestral",
  k_dpmpp_2m_sde: "DPM++ 2M SDE",
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

/** masonry 卡片的展示纵横比：模拟自然尺寸混排（竖/横/方），与官方版式节奏一致。 */
const QS_RATIOS = ["832/1216", "1216/832", "832/1216", "1024/1024", "832/1216", "1216/832"];

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
  desc?: string | undefined;
}) {
  return (
    <button
      type="button"
      class="nai-pop-item"
      data-selected={props.selected ? "true" : "false"}
      disabled={props.disabled}
      onClick={props.onSelect}
    >
      <span class="nai-pop-item-title">{props.children}</span>
      {props.desc ? <span class="nai-pop-item-desc">{props.desc}</span> : null}
    </button>
  );
}

/** 本页专用箭头：闭合朝下、展开朝上（官方口径；共享 ChevronIcon 是侧向的，留给旧版用）。 */
function NaiChevron({ size = 14, open = false, angle }: { size?: number; open?: boolean; angle?: number }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      aria-hidden="true"
      style={{
        transform: `rotate(${angle !== undefined ? angle : open ? 180 : 0}deg)`,
        transition: "transform 0.15s ease",
      }}
    >
      <path d="M6 9l6 6 6-6" stroke="currentColor" stroke-width="2" stroke-linecap="round" />
    </svg>
  );
}

/** 预设展示用英文口径，对齐参考站；数据 id 仍来自 workbench-config。 */
const QUALITY_EN: Record<QualityPreset, string> = { none: "None", light: "Light", standard: "Standard" };
const UC_EN: Record<UcPreset, string> = {
  none: "None",
  light: "Light",
  heavy: "Heavy",
  human: "Human Focus",
  furry: "Furry",
};

/* ---------------- 主应用 ---------------- */

export function NaiApp() {
  useEffect(() => start(), []);
  document.body.classList.add("nai-page");
  useEffect(() => () => document.body.classList.remove("nai-page"), []);
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
  const [promptTab, setPromptTab] = useState<"prompt" | "uc">("prompt");
  const [qualityPreset, setQualityPreset] = useState<QualityPreset>("standard");
  const [ucPreset, setUcPreset] = useState<UcPreset>("heavy");
  const [transparent, setTransparent] = useState(false);
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
  const [genSeed, setGenSeed] = useState<number | null>(null);
  const [notes, setNotes] = useState<string[]>([]);
  const [pinned, setPinned] = useState<number[]>([]);
  const [confirmClear, setConfirmClear] = useState(false);
  const [railCollapsed, setRailCollapsed] = useState(false);
  // 移动端（≤860px）：底部设置抽屉与全屏历史。桌面布局不渲染这两个入口。
  const [sheetOpen, setSheetOpen] = useState(false);
  const [histOpen, setHistOpen] = useState(false);
  const [pop, setPop] = useState<"" | "model" | "quality" | "uc" | "size" | "rescat" | "sampler">("");
  const [suggest, setSuggest] = useState<{ tags: string[] }>({ tags: [] });
  const [tokens, setTokens] = useState<number | null>(null);
  const [menuOpen, setMenuOpen] = useState(false);
  const menuTimer = useRef<number | null>(null);

  const abortRef = useRef<AbortController | null>(null);
  useEffect(() => () => abortRef.current?.abort(), []);

  const cycleTheme = () => {
    const order: WbTheme[] = ["dark", "sand", "slate"];
    const next = order[(order.indexOf(theme) + 1) % order.length] ?? "dark";
    setTheme(next);
    savePrefs({ ...loadPrefs(), theme: next });
  };

  const pickModel = (id: string) => {
    setModel(id);
    setPop("");
    setScale(isV5(id) ? 7 : 5);
    if (!samplersFor(id).includes(sampler)) setSampler(samplersFor(id)[0] ?? "k_euler");
  };

  const cycleMode = () => {
    const next = mode === "anime" ? "text" : "anime";
    setMode(next);
    toast(next === "anime" ? "已切换到 Anime 模式。" : "已切换到 Text 模式（本站标签建议仍按动漫口径）。");
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
  const swapSize = () => {
    setWidth(height);
    setHeight(width);
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

  /* token 计量：懒加载分词器，防抖更新（官方同款细条进度）。 */
  useEffect(() => {
    let alive = true;
    const text = promptTab === "prompt" ? prompt : negative;
    if (!text.trim()) {
      setTokens(0);
      return;
    }
    const timer = window.setTimeout(() => {
      tokenizeText(model, text)
        .then((t) => {
          if (alive) setTokens(t.ids.length);
        })
        .catch(() => {
          if (alive) setTokens(null);
        });
    }, 350);
    return () => {
      alive = false;
      window.clearTimeout(timer);
    };
  }, [prompt, negative, promptTab, model]);

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
    setGenSeed(resolveSeed(seed));
    const made: Shot[] = [];
    const baseIndex = shots.length;
    try {
      for (let i = 0; i < count; i++) {
        const used = count > 1 ? (resolveSeed(seed) + i) >>> 0 : resolveSeed(seed);
        setGenSeed(used);
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
        const fresh = imgs.map((img) => makeShot(img, attrs, count > 1 ? `${i + 1}/${count}` : undefined));
        made.push(...fresh);
        /* 出一张亮一张，和官方的增量出图一致 */
        setShots((prev) => [...prev, ...fresh]);
      }
      setViewing((prev) => (prev === null ? baseIndex : prev));
      toast(count > 1 ? `完成了 ${made.length} 张。` : "完成了。");
      void reloadMe();
    } catch (e) {
      if (e instanceof DOMException && e.name === "AbortError") return;
      toast(e instanceof Error ? e.message : "出图没有成功，稍后再试一次。", { error: true });
    } finally {
      setBusy(false);
      setGenSeed(null);
      abortRef.current = null;
    }
  };

  const shot = viewing !== null ? shots[viewing] : undefined;
  const anlas = member?.today?.anlas ?? 0;
  const usedToday = member?.today?.images ?? 0;
  const dailyImages = member?.today?.daily_images ?? member?.policy?.daily_images ?? 0;
  const quotaPct = dailyImages > 0 ? Math.min(100, Math.round((usedToday / dailyImages) * 100)) : 0;

  const copyShotPrompt = () => {
    if (!shot) return;
    navigator.clipboard
      .writeText(shot.attrs.prompt)
      .then(() => toast("提示词已复制。"))
      .catch(() => toast("复制失败，浏览器不允许剪贴板写入。", { error: true }));
  };

  const downloadAll = () => {
    if (!shots.length) return;
    void (async () => {
      try {
        const { zipSync } = await import("fflate");
        const files: Record<string, Uint8Array> = {};
        let i = 0;
        for (const s of shots) {
          const b64 = s.url.split(",")[1] ?? "";
          const bin = atob(b64);
          const bytes = new Uint8Array(bin.length);
          for (let j = 0; j < bin.length; j++) bytes[j] = bin.charCodeAt(j);
          files[`owl-${s.attrs.seed}-${i++}.${s.ext}`] = bytes;
        }
        const zipped = zipSync(files);
        const blob = new Blob([zipped.buffer as ArrayBuffer], { type: "application/zip" });
        const a = document.createElement("a");
        a.href = URL.createObjectURL(blob);
        a.download = "owl-history.zip";
        a.click();
        URL.revokeObjectURL(a.href);
        toast(`已打包 ${shots.length} 张。`);
      } catch {
        toast("打包下载失败，请逐张下载。", { error: true });
      }
    })();
  };

  const clearHistory = () => {
    if (!confirmClear) {
      setConfirmClear(true);
      window.setTimeout(() => setConfirmClear(false), 2500);
      return;
    }
    setShots([]);
    setViewing(null);
    setPinned([]);
    setConfirmClear(false);
    toast("已清空本次会话的历史。");
  };

  // 触屏点按也会先派发 mouseenter，只有支持悬停的设备才走 hover 开启。
  const canHover = typeof window.matchMedia === "function" && window.matchMedia("(hover: hover)").matches;
  const navMenuEnter = () => {
    if (!canHover) return;
    if (menuTimer.current) window.clearTimeout(menuTimer.current);
    setMenuOpen(true);
  };
  const navMenuLeave = () => {
    if (!canHover) return;
    if (menuTimer.current) window.clearTimeout(menuTimer.current);
    menuTimer.current = window.setTimeout(() => setMenuOpen(false), 220);
  };

  // 触屏没有 mouseleave：点菜单以外任意处收起。
  useEffect(() => {
    if (!menuOpen || canHover) return;
    const onDoc = (e: Event) => {
      const t = e.target as Element | null;
      if (!t?.closest(".nai-menu-wrap")) setMenuOpen(false);
    };
    document.addEventListener("click", onDoc, true);
    return () => document.removeEventListener("click", onDoc, true);
  }, [menuOpen]);

  const logout = async () => {
    try {
      const { sendJSON } = await import("../../../lib/api");
      await sendJSON("POST", "/logout");
      location.href = "/";
    } catch {
      toast("退出失败，请稍后再试。", { error: true });
    }
  };

  // 额度条与历史条目在桌面/移动两处复用（同一份状态，纯展示 + 提示）。
  const quotaBlock = (
    <div class="nai-quota">
      <div class="nai-quota-row">
        <span>
          {quotaPct}% 已用 · 今日剩 {Math.max(0, dailyImages - usedToday)} 张
        </span>
        <button
          type="button"
          class="nai-quota-more"
          onClick={() => toast("免费额度每日重置；V5 按策略另行计数。")}
        >
          More Info
        </button>
      </div>
      <div class="nai-quota-row2">
        <div class="nai-quota-track" aria-hidden="true">
          <i style={{ width: `${quotaPct}%` }} />
        </div>
        <button
          type="button"
          class="nai-quota-plus"
          title="充值"
          aria-label="充值"
          onClick={() => toast("充值暂未开放，出图额度由站长按日发放。", { error: true })}
        >
          <PlusIcon size={10} />
        </button>
      </div>
    </div>
  );
  const railItems = (
    <>
      {busy && (
        <div class="nai-rail-item nai-rail-item--spin" aria-hidden="true">
          <i />
        </div>
      )}
      {shots.map((s, i) => (
        <button
          key={`${s.attrs.seed}-${i}`}
          type="button"
          class="nai-rail-item"
          data-active={viewing === i ? "true" : "false"}
          title={`${s.attrs.seed} · ${s.attrs.width}×${s.attrs.height}`}
          onClick={() => {
            setViewing(i);
            setHistOpen(false);
          }}
        >
          <img src={s.url} alt={`历史 ${i + 1}`} loading="lazy" />
        </button>
      ))}
    </>
  );
  const railFoot = (
    <div class="nai-rail-foot">
      <button
        type="button"
        class="nai-rail-btn"
        title="锁定滚动"
        aria-label="锁定历史滚动"
        aria-pressed="false"
        onClick={() => toast("历史滚动锁定暂未开放。")}
      >
        <PinIcon size={14} />
      </button>
      <button
        type="button"
        class="nai-rail-btn"
        title="打包下载"
        aria-label="下载全部图片"
        onClick={downloadAll}
      >
        <Download size={14} />
      </button>
      <button
        type="button"
        class="nai-rail-btn"
        title={confirmClear ? "再点一次确认清空" : "清空历史"}
        aria-label="删除图片"
        data-danger={confirmClear ? "true" : "false"}
        onClick={clearHistory}
      >
        <TrashIcon size={14} />
      </button>
    </div>
  );

  return (
    <div class="nai-root" data-theme={theme}>
      <div class="nai-frame">
        {/* ============ 侧栏 ============ */}
        <aside class="nai-side">
          {/* 导航行：取消选择 / Anlas / 汉堡菜单 */}
          <div class="nai-nav-row">
            <button
              type="button"
              class="nai-nav-ghost"
              title="取消选择图片"
              aria-label="取消选择图片"
              onClick={() => setViewing(null)}
            >
              <SparkleIcon size={20} />
            </button>
            <div class="nai-anlas">
              <span class="nai-anlas-label">
                <span class="nai-anlas-pre">Anlas: </span>
                {anlas}
                <AnlasIcon size={11} />
              </span>
              <button
                type="button"
                class="nai-anlas-plus"
                title="充值"
                aria-label="充值"
                onClick={() => toast("充值暂未开放，出图额度由站长按日发放。", { error: true })}
              >
                <PlusIcon size={14} />
              </button>
            </div>
            <div class="nai-menu-wrap">
              <button
                type="button"
                class="nai-nav-btn"
                aria-label="菜单"
                aria-expanded={menuOpen ? "true" : "false"}
                onMouseEnter={navMenuEnter}
                onMouseLeave={navMenuLeave}
                onClick={() => setMenuOpen(!menuOpen)}
              >
                <MenuIcon size={18} />
              </button>
              {menuOpen && (
                <div
                  class="nai-menu-panel"
                  role="menu"
                  onMouseEnter={navMenuEnter}
                  onMouseLeave={navMenuLeave}
                >
                  <div class="nai-menu-group">Account</div>
                  {member ? (
                    <>
                      <button
                        type="button"
                        class="nai-menu-item"
                        onClick={() => toast("账号设置暂未开放。", { error: true })}
                      >
                        <User size={18} />
                        <span>Account Settings</span>
                      </button>
                      <button type="button" class="nai-menu-item" onClick={() => void logout()}>
                        <CloseIcon size={16} />
                        <span>Logout</span>
                      </button>
                    </>
                  ) : (
                    <a class="nai-menu-item" href="/login">
                      <LogIn size={18} />
                      <span>Discord Login</span>
                    </a>
                  )}
                  <div class="nai-menu-group">Others</div>
                  <button
                    type="button"
                    class="nai-menu-item"
                    onClick={() => {
                      setViewing(null);
                      setMenuOpen(false);
                    }}
                  >
                    <SparkleIcon size={16} />
                    <span>Quick Start Gallery</span>
                  </button>
                  <button
                    type="button"
                    class="nai-menu-item"
                    onClick={() => toast("导演工具（重绘/增强/改姿势）暂未开放。", { error: true })}
                  >
                    <Wand2 size={16} />
                    <span>Director Tools</span>
                  </button>
                  <button
                    type="button"
                    class="nai-menu-item"
                    onClick={() => toast("额度说明：免费额度每日重置，见下方进度条。")}
                  >
                    <AnlasIcon size={14} />
                    <span>Usage Limit</span>
                  </button>
                  <button
                    type="button"
                    class="nai-menu-item"
                    onClick={() => toast("库存（Inventory）暂未开放。", { error: true })}
                  >
                    <LayoutGrid size={16} />
                    <span>Inventory</span>
                  </button>
                  <button
                    type="button"
                    class="nai-menu-item"
                    onClick={() => toast("Tokenizer 页面暂未开放，实时计数见提示词卡下方。")}
                  >
                    <Pencil size={16} />
                    <span>Tokenizer</span>
                  </button>
                  <div class="nai-menu-group">Help</div>
                  <button
                    type="button"
                    class="nai-menu-item"
                    onClick={() => toast("使用教程暂未开放，先写提示词再点生成即可。")}
                  >
                    <PlayIcon size={16} />
                    <span>Tutorial</span>
                  </button>
                  <button type="button" class="nai-menu-item" onClick={cycleTheme}>
                    <SparkleIcon size={16} />
                    <span>Switch Theme (Dark / Sand / Slate)</span>
                  </button>
                </div>
              )}
            </div>
          </div>

          {/* 移动端顶部额度条（≤860 显示，桌面隐藏） */}
          <div class="nai-quota-bar">{quotaBlock}</div>

          {/* 滚动区（≤860 时变为底部设置抽屉） */}
          <div class={`nai-side-scroll${sheetOpen ? " nai-side-scroll--open" : ""}`}>
            {/* Model / Mode */}
            <div class="nai-model-row">
              <div class="nai-model-cell">
                <span class="nai-row-label">Model</span>
                <div class="nai-pop">
                  <button
                    type="button"
                    class="nai-pill"
                    onClick={() => setPop(pop === "model" ? "" : "model")}
                    aria-expanded={pop === "model" ? "true" : "false"}
                  >
                    <span>{modelOf(model).label}</span>
                    <NaiChevron size={14} open={pop === "model"} />
                  </button>
                  <Pop open={pop === "model"} onClose={() => setPop("")}>
                    <div class="nai-pop-cap">New</div>
                    {V5_MODELS.map((m) => (
                      <PopItem
                        key={m.id}
                        selected={model === m.id}
                        disabled={!v5Open}
                        desc={v5Open ? MODEL_BLURBS[m.id]?.blurb : "需要站长为你的 Key 开放 V5。"}
                        onSelect={() => pickModel(m.id)}
                      >
                        {m.label}
                      </PopItem>
                    ))}
                    <div class="nai-pop-cap">Legacy</div>
                    {BASE_MODELS.map((m) => (
                      <PopItem
                        key={m.id}
                        selected={model === m.id}
                        desc={MODEL_BLURBS[m.id]?.blurb}
                        onSelect={() => pickModel(m.id)}
                      >
                        {m.label}
                      </PopItem>
                    ))}
                  </Pop>
                </div>
              </div>
              <div class="nai-model-cell">
                <span class="nai-row-label">Mode</span>
                <button type="button" class="nai-pill nai-mode-btn" onClick={cycleMode}>
                  <SparkleIcon size={14} />
                  <span>{mode === "anime" ? "Anime" : "Text"}</span>
                </button>
              </div>
            </div>

            {/* Prompt / Undesired Content 标签卡 */}
            <div class="nai-card">
              <div class="nai-card-head">
                <div class="nai-tabs">
                  <div class="nai-tab" data-active={promptTab === "prompt" ? "true" : "false"}>
                    <button type="button" onClick={() => setPromptTab("prompt")}>
                      Prompt
                    </button>
                  </div>
                  <div class="nai-tab" data-active={promptTab === "uc" ? "true" : "false"}>
                    <button type="button" onClick={() => setPromptTab("uc")}>
                      Undesired Content
                    </button>
                  </div>
                </div>
                <div class="nai-card-actions">
                  <button
                    type="button"
                    class="nai-sq-btn"
                    title="全屏编辑器"
                    aria-label="全屏编辑器"
                    onClick={() => toast("全屏提示词编辑器暂未开放。", { error: true })}
                  >
                    <ExpandIcon size={16} />
                  </button>
                  <button
                    type="button"
                    class="nai-sq-btn"
                    title="交换提示词与负面"
                    aria-label="交换提示词与负面"
                    onClick={() => {
                      const p = prompt;
                      setPrompt(negative);
                      setNegative(p);
                    }}
                  >
                    <SwapIcon size={16} />
                  </button>
                </div>
              </div>
              {promptTab === "prompt" ? (
                <div class="nai-card-pane">
                  <div class="nai-card-editor">
                    <textarea
                      class="nai-textarea"
                      aria-label="提示词"
                      placeholder="写下你想画的内容，比如：1girl, sunset, very aesthetic"
                      value={prompt}
                      onInput={(e) => setPrompt((e.target as HTMLTextAreaElement).value)}
                    />
                    {suggest.tags.length > 0 && (
                      <div class="nai-suggest">
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
                    )}
                  </div>
                  <div class="nai-card-foot">
                    <button
                      type="button"
                      class="nai-toggle"
                      aria-pressed={transparent ? "true" : "false"}
                      title="透明背景"
                      onClick={() => {
                        if (model.includes("diffusion-3")) toast("这个模型不支持透明背景输出。");
                        else setTransparent(!transparent);
                      }}
                    >
                      {transparent ? <CheckIcon size={12} /> : <CloseIcon size={12} />}
                      <span>Transparent BG</span>
                    </button>
                    <div class="nai-pop">
                      <button
                        type="button"
                        class="nai-miniselect"
                        onClick={() => setPop(pop === "quality" ? "" : "quality")}
                      >
                        <span>Quality Tags: {QUALITY_EN[qualityPreset]}</span>
                        <NaiChevron size={12} />
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
                            {QUALITY_EN[q]}
                          </PopItem>
                        ))}
                      </Pop>
                    </div>
                    <div class="nai-tokenbar" aria-hidden="true">
                      <i style={{ width: `${tokens === null ? 0 : Math.min(100, (tokens / 512) * 100)}%` }} />
                    </div>
                  </div>
                </div>
              ) : (
                <div class="nai-card-pane">
                  <div class="nai-card-editor">
                    <textarea
                      class="nai-textarea"
                      aria-label="负面提示词"
                      placeholder="不希望出现的内容（低质量词由预设自动带上）"
                      value={negative}
                      onInput={(e) => setNegative((e.target as HTMLTextAreaElement).value)}
                    />
                  </div>
                  <div class="nai-card-foot">
                    <div class="nai-pop">
                      <button
                        type="button"
                        class="nai-miniselect"
                        onClick={() => setPop(pop === "uc" ? "" : "uc")}
                      >
                        <span>UC Preset: {UC_EN[ucPreset]}</span>
                        <NaiChevron size={12} />
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
                            {UC_EN[u]}
                          </PopItem>
                        ))}
                      </Pop>
                    </div>
                  </div>
                </div>
              )}
            </div>

            {/* 角色提示词（未开放） */}
            <div class="nai-card nai-card--box">
              <div class="nai-char-row">
                <div>
                  <div class="nai-char-title">Character Prompts</div>
                  <div class="nai-char-sub">为场景中的角色分别写提示词。</div>
                </div>
                <button
                  type="button"
                  class="nai-plus-btn"
                  title="添加角色"
                  aria-label="添加角色"
                  onClick={() => toast("角色提示词暂未开放，多角色请直接写在提示词里。", { error: true })}
                >
                  <PlusIcon size={16} />
                </button>
              </div>
              <div class="nai-char-pos">
                <span class="nai-row-label">Position</span>
                <div class="nai-seg nai-seg--pos">
                  <button
                    type="button"
                    data-active="false"
                    onClick={() => toast("角色定位随角色提示词一起，暂未开放。", { error: true })}
                  >
                    AI's Choice
                  </button>
                  <button
                    type="button"
                    data-active="false"
                    onClick={() => toast("角色定位随角色提示词一起，暂未开放。", { error: true })}
                  >
                    Custom
                  </button>
                  <button
                    type="button"
                    class="nai-seg-icon"
                    aria-label="自定义位置"
                    onClick={() => toast("角色定位随角色提示词一起，暂未开放。", { error: true })}
                  >
                    <SquareIcon size={14} />
                  </button>
                </div>
              </div>
            </div>

            {/* Reference Images / Image2Image（未开放） */}
            <div class="nai-group-label">Reference Images</div>
            <div class="nai-i2i">
              <span class="nai-i2i-icon">
                <UploadIcon size={16} />
              </span>
              <div class="nai-i2i-text">
                <div class="nai-char-title">Image2Image</div>
                <div class="nai-char-sub">以参考图为基础变换画面。</div>
              </div>
              <button
                type="button"
                class="nai-sq-btn nai-sq-btn--dim"
                title="上传参考图"
                aria-label="上传参考图"
                onClick={() => toast("图生图暂未开放。", { error: true })}
              >
                <UploadIcon size={16} />
              </button>
              <button
                type="button"
                class="nai-sq-btn nai-sq-btn--dim"
                title="编辑参考图"
                aria-label="编辑参考图"
                onClick={() => toast("图生图暂未开放。", { error: true })}
              >
                <Pencil size={16} />
              </button>
            </div>

            {/* Image Settings */}
            <div class="nai-group-label">Image Settings</div>
            <div class="nai-settings">
              <div class="nai-res-row">
                <span class="nai-res-title">Resolution</span>
                <div class="nai-res-pill">
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
                    class="nai-res-swap"
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
              <div class="nai-res-row2">
                <div class="nai-pop">
                  <button
                    type="button"
                    class="nai-pill nai-pill--cat"
                    onClick={() => setPop(pop === "rescat" ? "" : "rescat")}
                  >
                    <span>{SIZE_PRESETS.find((s) => s.id === presetId)?.label ?? "自定义"}</span>
                    <NaiChevron size={12} open={pop === "rescat"} />
                  </button>
                  <Pop open={pop === "rescat"} onClose={() => setPop("")}>
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
                <div class="nai-seg nai-seg--orient">
                  <button
                    type="button"
                    aria-label="横图"
                    data-active={orientOf() === "landscape" ? "true" : "false"}
                    onClick={() => applyOrient("landscape")}
                  >
                    <LandscapeIcon size={15} />
                  </button>
                  <button
                    type="button"
                    aria-label="竖图"
                    data-active={orientOf() === "portrait" ? "true" : "false"}
                    onClick={() => applyOrient("portrait")}
                  >
                    <PortraitIcon size={15} />
                  </button>
                  <button
                    type="button"
                    aria-label="方图"
                    data-active={orientOf() === "square" ? "true" : "false"}
                    onClick={() => applyOrient("square")}
                  >
                    <SquareIcon size={15} />
                  </button>
                </div>
              </div>
              <div>
                <div class="nai-res-title">Number of Images</div>
                <div class="nai-seg nai-seg--count">
                  {[1, 2, 3, 4].map((n) => (
                    <button
                      key={n}
                      type="button"
                      data-active={count === n ? "true" : "false"}
                      onClick={() => setCount(n)}
                    >
                      {n}
                    </button>
                  ))}
                </div>
              </div>
            </div>
          </div>

          {/* 移动端折叠提示词条：点开底部设置抽屉（≤860 显示） */}
          <button
            type="button"
            class="nai-mbar"
            aria-expanded={sheetOpen ? "true" : "false"}
            onClick={() => setSheetOpen(!sheetOpen)}
          >
            <span class="nai-mbar-text" data-empty={prompt ? undefined : "true"}>
              {prompt || "Type your prompt here…"}
            </span>
            <NaiChevron size={14} angle={sheetOpen ? 0 : 180} />
          </button>

          {/* footer：参数行 / 额度条 / 生成 */}
          <div class="nai-side-foot">
            <div class="nai-params">
              <div class="nai-params-grid">
                <label class="nai-param">
                  <span>Steps</span>
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
                </label>
                <label class="nai-param">
                  <span>Guidance</span>
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
                </label>
                <div class="nai-param">
                  <span>Seed</span>
                  <div class="nai-param-seed">
                    <input
                      type="text"
                      inputmode="numeric"
                      aria-label="种子"
                      placeholder="-1"
                      value={seed}
                      onInput={(e) => setSeed((e.target as HTMLInputElement).value)}
                    />
                    <button
                      type="button"
                      class="nai-param-icon"
                      title="使用当前图片的种子"
                      aria-label="使用当前图片的种子"
                      disabled={!shot}
                      onClick={() => {
                        if (shot) setSeed(String(shot.attrs.seed));
                      }}
                    >
                      <SeedIcon size={14} />
                    </button>
                  </div>
                </div>
                <div class="nai-param nai-param--sampler">
                  <span>Sampler</span>
                  <div class="nai-pop">
                    <button
                      type="button"
                      class="nai-param-value"
                      onClick={() => setPop(pop === "sampler" ? "" : "sampler")}
                    >
                      {samplerLabel(sampler)}
                    </button>
                    <Pop open={pop === "sampler"} onClose={() => setPop("")}>
                      <div class="nai-pop-cap">Recommended</div>
                      <PopItem
                        selected={sampler === "k_euler_ancestral"}
                        onSelect={() => {
                          setSampler("k_euler_ancestral");
                          setPop("");
                        }}
                      >
                        Euler Ancestral
                      </PopItem>
                      <div class="nai-pop-cap">Other</div>
                      {samplersFor(model)
                        .filter((s) => s !== "k_euler_ancestral")
                        .map((s) => (
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
              <button
                type="button"
                class="nai-play"
                title="生成"
                aria-label="生成"
                disabled={busy}
                onClick={() => void generate()}
              >
                <PlayIcon size={16} />
              </button>
            </div>

            {quotaBlock}

            {notes.length > 0 && (
              <div class="nai-status" role="status">
                {notes.join("；")}
              </div>
            )}
            <div class="nai-generate-row">
              <button
                type="button"
                class="nai-foot-icon nai-foot-gear"
                title="生成设置"
                aria-label="生成设置"
                onClick={() => setSheetOpen(true)}
              >
                <Settings size={16} />
              </button>
              <button type="button" class="nai-generate" disabled={busy} onClick={() => void generate()}>
                <span>{`Generate ${count} Image${count > 1 ? "s" : ""}`}</span>
                <span class="nai-generate-badge">
                  {busy ? "…" : "0"}
                  <AnlasIcon size={10} />
                </span>
              </button>
              <button
                type="button"
                class="nai-foot-icon nai-foot-hist"
                title="历史图片"
                aria-label="历史图片"
                onClick={() => setHistOpen(true)}
              >
                <Clock size={16} />
              </button>
            </div>
          </div>
        </aside>

        {/* ============ 画布 ============ */}
        <main class="nai-canvas">
          {busy && (
            <div class="nai-genbar" aria-hidden="true">
              <i />
            </div>
          )}
          {shot ? (
            <div class="nai-view">
              <div class="nai-view-top">
                <button
                  type="button"
                  class="nai-tool"
                  title="增强"
                  aria-label="增强"
                  onClick={() => toast("增强（Enhance）暂未开放。", { error: true })}
                >
                  <SparkleIcon size={18} />
                </button>
                <button
                  type="button"
                  class="nai-tool nai-tool--wide"
                  title="生成变体"
                  aria-label="生成变体"
                  onClick={() => toast("变体（Variations）暂未开放。", { error: true })}
                >
                  <LayoutGrid size={16} />
                  <AnlasIcon size={11} />
                </button>
                <button
                  type="button"
                  class="nai-tool nai-tool--wide"
                  title="放大"
                  aria-label="放大"
                  onClick={() => toast("放大（Upscale）暂未开放。", { error: true })}
                >
                  <ExpandIcon size={16} />
                  <AnlasIcon size={11} />
                </button>
                <button
                  type="button"
                  class="nai-tool"
                  title="用作初始图"
                  aria-label="用作初始图"
                  onClick={() => toast("图生图暂未开放，无法把这张图作为初始图。", { error: true })}
                >
                  <UploadIcon size={17} />
                </button>
                <button
                  type="button"
                  class="nai-tool"
                  title="在编辑器中打开"
                  aria-label="在编辑器中打开"
                  onClick={() => toast("编辑器暂未开放。", { error: true })}
                >
                  <Pencil size={17} />
                </button>
                <button
                  type="button"
                  class="nai-tool"
                  title="局部重绘"
                  aria-label="局部重绘"
                  onClick={() => toast("局部重绘（Inpaint）暂未开放。", { error: true })}
                >
                  <Wand2 size={17} />
                </button>
                <button
                  type="button"
                  class="nai-tool"
                  title="发送到工具"
                  aria-label="发送到工具"
                  onClick={() => toast("外部工具入口暂未开放。", { error: true })}
                >
                  <ArrowRight size={17} />
                </button>
              </div>
              <div class="nai-view-stage">
                <img class="nai-view-img" src={shot.url} alt={`生成结果 ${shot.attrs.seed}`} />
              </div>
              <div class="nai-view-bottom">
                <div class="nai-view-bottom-left">
                  <div class="nai-res-pill">
                    <input
                      type="number"
                      min={64}
                      max={3072}
                      aria-label="宽"
                      value={width}
                      onInput={(e) => {
                        setWidth(Number((e.target as HTMLInputElement).value) || 64);
                        setPresetId("custom");
                      }}
                    />
                    <span class="nai-res-x">×</span>
                    <input
                      type="number"
                      min={64}
                      max={3072}
                      aria-label="高"
                      value={height}
                      onInput={(e) => {
                        setHeight(Number((e.target as HTMLInputElement).value) || 64);
                        setPresetId("custom");
                      }}
                    />
                  </div>
                  <button
                    type="button"
                    class="nai-tool"
                    title="画布显示设置"
                    aria-label="画布显示设置"
                    onClick={() => toast("画布显示设置暂未开放。", { error: true })}
                  >
                    <Settings size={16} />
                  </button>
                  <button
                    type="button"
                    class="nai-seedpill"
                    title="复制到种子框"
                    onClick={() => {
                      setSeed(String(shot.attrs.seed));
                      toast(`已填入种子 ${shot.attrs.seed}。`);
                    }}
                  >
                    <SeedIcon size={14} />
                    <span>{genSeed ?? shot.attrs.seed}</span>
                  </button>
                </div>
                <div class="nai-view-bottom-right">
                  <button
                    type="button"
                    class="nai-tool"
                    title="收藏"
                    aria-label="收藏"
                    aria-pressed={viewing !== null && pinned.includes(viewing) ? "true" : "false"}
                    onClick={() =>
                      setPinned((prev) =>
                        viewing === null || prev.includes(viewing)
                          ? prev.filter((i) => i !== viewing)
                          : [...prev, viewing],
                      )
                    }
                  >
                    <PinIcon size={17} />
                  </button>
                  <button
                    type="button"
                    class="nai-tool"
                    title="复制提示词"
                    aria-label="复制提示词"
                    onClick={copyShotPrompt}
                  >
                    <CopyIcon size={17} />
                  </button>
                  <a
                    class="nai-tool"
                    title="下载"
                    aria-label="下载图片"
                    href={shot.url}
                    download={`owl-${shot.attrs.seed}.${shot.ext}`}
                  >
                    <Download size={17} />
                  </a>
                </div>
              </div>
            </div>
          ) : (
            <div class="nai-gallery">
              <div class="nai-hero">
                <svg class="nai-hero-art" viewBox="0 0 691 258" aria-hidden="true">
                  <defs>
                    <linearGradient id="nai-hero-g" x1="0" y1="0" x2="1" y2="1">
                      <stop offset="0" stop-color="var(--nai-accent)" stop-opacity="0.9" />
                      <stop offset="1" stop-color="var(--nai-accent)" stop-opacity="0.05" />
                    </linearGradient>
                  </defs>
                  <g
                    fill="none"
                    stroke="url(#nai-hero-g)"
                    stroke-width="2.5"
                    stroke-linecap="round"
                    opacity="0.5"
                  >
                    <path d="M320 130 C 350 70, 420 60, 460 110" />
                    <path d="M240 180 C 300 150, 350 160, 390 200" />
                    <path d="M480 190 C 520 150, 570 150, 610 190" />
                    <path d="M150 90 C 180 60, 220 60, 250 85" />
                  </g>
                  <g fill="var(--nai-accent)">
                    <path d="M350 52 l6 14 14 6 -14 6 -6 14 -6 -14 -14 -6 14 -6 Z" opacity="0.9" />
                    <path
                      d="M560 70 l4.5 10.5 10.5 4.5 -10.5 4.5 -4.5 10.5 -4.5 -10.5 -10.5 -4.5 10.5 -4.5 Z"
                      opacity="0.75"
                    />
                    <path d="M240 40 l3.5 8 8 3.5 -8 3.5 -3.5 8 -3.5 -8 -8 -3.5 8 -3.5 Z" opacity="0.7" />
                  </g>
                  <g fill="var(--nai-panel)" opacity="0.85">
                    <path d="M80 60 l14 32 32 14 -32 14 -14 32 -14 -32 -32 -14 32 -14 Z" />
                    <path d="M600 200 l10 24 24 10 -24 10 -10 24 -10 -24 -24 -10 24 -10 Z" opacity="0.7" />
                  </g>
                </svg>
                <h1 class="nai-hero-title">Get Started</h1>
                <p class="nai-hero-sub">Get Inspiration from our quick start gallery!</p>
                <p class="nai-hero-hint">Click an image to copy the prompt.</p>
              </div>
              <div class="nai-qs-masonry">
                {QUICKSTART.slice(0, 12).map((card, i) => (
                  <QsCard key={card.id} index={i} prompt={card.prompt} />
                ))}
              </div>
              {busy && (
                <div class="nai-placeholder" aria-hidden="true">
                  <i />
                </div>
              )}
            </div>
          )}
        </main>

        {/* ============ 右轨 History ============ */}
        <aside class={`nai-rail${railCollapsed ? " nai-rail--collapsed" : ""}`}>
          <div class="nai-rail-head">
            <button
              type="button"
              class="nai-rail-toggle"
              aria-expanded={railCollapsed ? "false" : "true"}
              onClick={() => setRailCollapsed(!railCollapsed)}
            >
              <span>History</span>
              <NaiChevron size={12} angle={railCollapsed ? 0 : -90} />
            </button>
          </div>
          {!railCollapsed && (
            <>
              <div class="nai-rail-list">{railItems}</div>
              {railFoot}
            </>
          )}
        </aside>
      </div>

      {/* 移动端：设置抽屉遮罩 + 全屏历史（入口仅 ≤860 渲染） */}
      {sheetOpen && (
        <button
          type="button"
          class="nai-scrim"
          aria-label="关闭设置抽屉"
          onClick={() => setSheetOpen(false)}
        />
      )}
      {histOpen && (
        <div class="nai-hist" role="dialog" aria-modal="true" aria-label="历史图片">
          <div class="nai-hist-head">
            <span>History</span>
            <button
              type="button"
              class="nai-hist-close"
              aria-label="关闭历史"
              onClick={() => setHistOpen(false)}
            >
              <CloseIcon size={18} />
            </button>
          </div>
          <div class="nai-hist-list">{railItems}</div>
          {railFoot}
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
    <button
      type="button"
      class="nai-qs-card"
      style={{ aspectRatio: QS_RATIOS[props.index % QS_RATIOS.length] ?? "832/1216" }}
      title="点击复制提示词"
      onClick={() => void copy()}
    >
      <span class="nai-qs-art" style={{ background: qsArt(props.index) }} aria-hidden="true" />
      <span class="nai-qs-copy">
        {copied ? <CheckIcon size={16} /> : <CopyIcon size={16} />}
        {copied ? "已复制" : "Copied!"}
      </span>
    </button>
  );
}
