/**
 * 生图工作台（Beta）：整页出图界面。布局骨架取自 nai-workbench（左输入 / 中舞台 / 右画廊），
 * 观感与组件用本站 kit 重写；数据层在 workbench-config.ts（纯逻辑）与 api.ts（网络）。
 * 「批量出图」页共用同一份模型与尺寸参数（Batch.tsx），画廊两页共用。
 * 提示都写在页面上：出图计入今日额度、取消不一定免费、画廊刷新即清空。
 */
import { useEffect, useRef, useState } from "preact/hooks";
import {
  BASE_MODELS,
  clampForm,
  costHint,
  DEFAULT_FORM,
  FREE_MAX_PIXELS,
  FREE_MAX_STEPS,
  hasSchedule,
  isV3,
  modelOf,
  NOISE_SCHEDULES,
  type PaintForm,
  resolveSeed,
  SIZE_PRESETS,
  samplersFor,
  V5_MODELS,
} from "../../lib/nai-config";
import { Badge, Button, Card, cx, Dialog, Field, Notice, Skeleton, toast } from "../../ui";
import type { MeMember } from "../landing/types";
import type { Shot } from "./api";
import { fetchSuggestions, makeShot, postImages } from "./api";
import { BatchPane } from "./Batch";
import { FieldBlock } from "./bits";
import { me as meSignal, reloadMe, start } from "./store";
import {
  type BatchGroup,
  type BatchUnified,
  buildTaskRequest,
  composeNegative,
  composePositive,
  defaultGroups,
  estimateTokens,
  MAX_FILE_BYTES,
  MAX_REFS,
  MODE_LABELS,
  modesFor,
  type QualityPreset,
  qualityPresetList,
  type RefImage,
  type SourceImage,
  type Strings,
  tokenLimit,
  UC_LABELS,
  type UcPreset,
  ucPresetList,
  type WbMode,
} from "./workbench-config";
import "./workbench.css";

/** 出图页的完整草稿：参数沿用 PaintForm，另加预设 / 模式 / 图参。 */
type DrawState = {
  form: PaintForm;
  qualityPreset: QualityPreset;
  ucPreset: UcPreset;
  mode: WbMode;
  artist: string;
  source: SourceImage | null;
  mask: SourceImage | null;
  strength: number;
  noise: number;
  refs: RefImage[];
  suggest: { busy: boolean; tags: string[] };
};

const INITIAL_DRAW: DrawState = {
  form: { ...DEFAULT_FORM },
  qualityPreset: "none",
  ucPreset: "light",
  mode: "generate",
  artist: "",
  source: null,
  mask: null,
  strength: 0.7,
  noise: 0,
  refs: [],
  suggest: { busy: false, tags: [] },
};

export function WorkbenchApp() {
  useEffect(() => start(), []);
  const me = meSignal.value;
  if (me === undefined) {
    return (
      <div class="wb wb--loading">
        <Card title="生图工作台">
          <Skeleton h="2em" /> <Skeleton w="60%" h="2em" />
        </Card>
      </div>
    );
  }
  if (!me.logged_in) {
    return (
      <div class="wb wb--loading">
        <Card title="生图工作台（Beta）" sub="这里是成员的网页出图界面。">
          <Notice tone="warn">先登录再看这里——工作台按你的 Key 出图和记账。</Notice>
          <p>
            <a class="btn btn--primary" href="/login">
              用 Discord 登录
            </a>
          </p>
        </Card>
      </div>
    );
  }
  return <Workbench me={me} />;
}

function Workbench({ me }: { me: MeMember }) {
  const paid = me.policy?.paid_parameters !== false;
  const v5Open = me.policy?.v5 === true;
  const tagsOpen = me.policy?.tags === true;
  const vibeOpen = me.policy?.vibe === true;
  const blocked = entryProblem(me);

  const [tab, setTab] = useState<"draw" | "batch">("draw");
  const [draw, setDraw] = useState<DrawState>(() => ({ ...INITIAL_DRAW }));
  const patch = (p: Partial<DrawState>) => setDraw((d) => ({ ...d, ...p }));
  const patchForm = (p: Partial<PaintForm>) => setDraw((d) => ({ ...d, form: { ...d.form, ...p } }));

  const [history, setHistory] = useState<Shot[]>([]);
  const addShots = (shots: Shot[]) => setHistory((h) => [...shots, ...h].slice(0, 20));
  const [viewing, setViewing] = useState<Shot | null>(null);

  // 批量页的草稿也放在这一层：切页签不丢
  const [shared, setShared] = useState<Strings>({ artist: "", quality: "", negative: "" });
  const [unified, setUnified] = useState<BatchUnified>({ artist: true, quality: true, negative: true });
  const [groups, setGroups] = useState<BatchGroup[]>(defaultGroups);

  if (blocked) {
    return (
      <div class="wb wb--loading">
        <Card title="生图工作台（Beta）">
          <Notice tone="warn">{blocked}</Notice>
          <p>
            <a class="btn" href="/">
              回首页
            </a>
          </p>
        </Card>
      </div>
    );
  }

  const today = me.today;
  const quota = today
    ? `今日 ${today.images}/${today.daily_images} 张 · V5 ${today.v5}/${today.daily_v5}`
    : null;

  return (
    <div class="wb">
      <header class="wb-head">
        <div class="wb-head__l">
          <a class="wb-back" href="/">
            ← 首页
          </a>
          <h1>
            生图工作台 <Badge tone="accent">Beta</Badge>
          </h1>
        </div>
        <div class="wb-head__r">
          {quota && <span class="wb-quota">{quota}</span>}
          <span class="wb-who">{displayName(me)}</span>
        </div>
      </header>
      <p class="wb-beta muted small">
        测试版：界面和参数还会调整，欢迎在 Discord
        提意见。画廊与草稿只在当前页面，刷新即清空；出图计入今天的额度。
      </p>
      <div class="wb-tabs" role="tablist" aria-label="工作台页签">
        <button type="button" role="tab" aria-selected={tab === "draw"} onClick={() => setTab("draw")}>
          出图
        </button>
        <button type="button" role="tab" aria-selected={tab === "batch"} onClick={() => setTab("batch")}>
          批量出图
        </button>
      </div>
      <div class="wb-body">
        <DrawPane
          hidden={tab !== "draw"}
          me={me}
          draw={draw}
          patch={patch}
          patchForm={patchForm}
          paid={paid}
          v5Open={v5Open}
          tagsOpen={tagsOpen}
          vibeOpen={vibeOpen}
          addShots={addShots}
          openShot={setViewing}
        />
        <BatchPane
          hidden={tab !== "batch"}
          base={draw.form}
          presets={{ qualityPreset: draw.qualityPreset, ucPreset: draw.ucPreset }}
          mode={draw.mode}
          paid={paid}
          apiKey={me.key ?? ""}
          shared={shared}
          setShared={setShared}
          unified={unified}
          setUnified={setUnified}
          groups={groups}
          setGroups={setGroups}
          addShots={addShots}
          openShot={setViewing}
        />
        <HistoryRail shots={history} onOpen={setViewing} />
      </div>
      <ShotDialog
        shot={viewing}
        onClose={() => setViewing(null)}
        onReuse={(seed) => {
          patchForm({ seed: String(seed) });
          setTab("draw");
          setViewing(null);
          toast("种子已带回「出图」页。");
        }}
      />
    </div>
  );
}

/* ---------------- 出图页 ---------------- */

function DrawPane(props: {
  hidden: boolean;
  me: MeMember;
  draw: DrawState;
  patch: (p: Partial<DrawState>) => void;
  patchForm: (p: Partial<PaintForm>) => void;
  paid: boolean;
  v5Open: boolean;
  tagsOpen: boolean;
  vibeOpen: boolean;
  addShots: (shots: Shot[]) => void;
  openShot: (s: Shot) => void;
}) {
  const { me, draw, patch, patchForm, paid, v5Open, tagsOpen, vibeOpen } = props;
  const abort = useRef<AbortController | null>(null);
  const [running, setRunning] = useState(false);
  const [elapsed, setElapsed] = useState(0);
  const [note, setNote] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [shot, setShot] = useState<Shot | null>(null);

  useEffect(() => {
    if (!running) return;
    setElapsed(0);
    const t = setInterval(() => setElapsed((s) => s + 1), 1000);
    return () => clearInterval(t);
  }, [running]);

  const modes = modesFor(me.policy);
  const models = v5Open ? [...BASE_MODELS, ...V5_MODELS] : BASE_MODELS;
  const samplers = samplersFor(draw.form.model);

  const chooseModel = (id: string) => {
    const keep = samplersFor(id).includes(draw.form.sampler)
      ? draw.form.sampler
      : (samplersFor(id)[0] ?? "k_euler_ancestral");
    const ucs = ucPresetList(id);
    const uc = ucs.includes(draw.ucPreset) ? draw.ucPreset : "none";
    const qs = qualityPresetList(id);
    const q = qs.includes(draw.qualityPreset) ? draw.qualityPreset : "none";
    patchForm({ model: id, sampler: keep, smea: isV3(id) ? draw.form.smea : false });
    patch({ ucPreset: uc, qualityPreset: q, mode: modes.includes(draw.mode) ? draw.mode : "generate" });
  };

  const choosePreset = (id: string) => {
    const p = SIZE_PRESETS.find((x) => x.id === id);
    patchForm(p?.w && p?.h ? { preset: id, width: p.w, height: p.h } : { preset: id });
  };

  const suggestTags = async () => {
    if (draw.suggest.busy || !me.key) return;
    const prompt = draw.form.positive.trim();
    if (!prompt) {
      toast("先写几个开头（人物、场景都行），再点补全。");
      return;
    }
    patch({ suggest: { busy: true, tags: [] } });
    try {
      const tags = await fetchSuggestions(prompt, draw.form.model, me.key, new AbortController().signal);
      if (!tags.length) toast("这次没拿到标签建议，换个写法再试。");
      else patch({ suggest: { busy: false, tags } });
    } catch (e) {
      toast(e instanceof Error ? e.message : "标签补全没有成功，请稍后再试。", { error: true });
      patch({ suggest: { busy: false, tags: [] } });
    }
  };

  const appendTag = (tag: string) => {
    const cur = draw.form.positive.trim();
    if (new RegExp(`(^|,\\s*)${tag.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}(\\s*,|$)`).test(cur)) return;
    patchForm({ positive: cur ? `${cur}, ${tag}` : tag });
  };

  const readImage = (file: File): Promise<SourceImage> =>
    new Promise((resolve, reject) => {
      if (file.size > MAX_FILE_BYTES) {
        reject(new Error(`「${file.name}」超过 12 MiB，先压缩再传。`));
        return;
      }
      const r = new FileReader();
      r.onerror = () => reject(new Error("这张图读不出来。"));
      r.onload = () => {
        const url = String(r.result ?? "");
        const comma = url.indexOf(",");
        if (!url.startsWith("data:") || comma < 0) {
          reject(new Error("这张图读不出来。"));
          return;
        }
        resolve({ name: file.name, dataUrl: url, b64: url.slice(comma + 1) });
      };
      r.readAsDataURL(file);
    });

  const pickSource = async (files: FileList | null) => {
    const file = files?.[0];
    if (!file) return;
    try {
      patch({ source: await readImage(file) });
    } catch (e) {
      toast(e instanceof Error ? e.message : "这张图读不出来。", { error: true });
    }
  };
  const pickMask = async (files: FileList | null) => {
    const file = files?.[0];
    if (!file) return;
    try {
      patch({ mask: await readImage(file) });
    } catch (e) {
      toast(e instanceof Error ? e.message : "这张图读不出来。", { error: true });
    }
  };
  const pickRefs = async (files: FileList | null) => {
    const list = [...(files ?? [])].slice(0, MAX_REFS);
    if (!list.length) return;
    try {
      const imgs = await Promise.all(list.map(readImage));
      patch({
        refs: imgs.map((img, i) => ({
          id: `${Date.now()}-${i}`,
          name: img.name,
          dataUrl: img.dataUrl,
          b64: img.b64,
          strength: 0.6,
        })),
      });
    } catch (e) {
      toast(e instanceof Error ? e.message : "图片没读出来。", { error: true });
    }
  };

  const generate = async () => {
    if (running) return;
    const prompt = draw.form.positive.trim();
    if (!prompt) {
      setError("先写点提示词吧，比如：1girl, sunset, very aesthetic");
      return;
    }
    if (draw.mode !== "generate" && !draw.source) {
      setError("这个模式要先传一张原图。");
      return;
    }
    if (draw.mode === "infill" && !draw.mask) {
      setError("局部重绘还要传蒙版：白色是要重画的地方，黑色保留。");
      return;
    }
    setError(null);
    // 组合一次并冻结：请求、属性面板、画廊看到的都是同一份组合结果
    const positive = composePositive(draw.form.model, prompt, draw.artist, draw.qualityPreset);
    const negative = composeNegative(draw.form.model, draw.form.negative, draw.ucPreset, positive);
    const taskForm: PaintForm = { ...draw.form, positive, negative };
    const { form: send, notes } = clampForm(taskForm, paid);
    const seed = resolveSeed(send.seed);
    const body = buildTaskRequest(
      {
        form: send,
        mode: draw.mode,
        source: draw.source,
        mask: draw.mask,
        strength: draw.strength,
        noise: draw.noise,
        refs: draw.refs.map((r) => ({ image: r.b64, strength: r.strength })),
      },
      seed,
    );
    setNote(notes.length ? notes.join("；") : null);
    setRunning(true);
    const controller = new AbortController();
    abort.current = controller;
    const startedAt = Date.now();
    try {
      const imgs = await postImages(body, me.key ?? "", controller.signal);
      const first = imgs[0];
      if (!first) throw new Error("返回里没有图片。");
      const next = makeShot(first, {
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
      });
      setShot(next);
      props.addShots([next]);
      void reloadMe();
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

  const overPixels = !paid && draw.form.width * draw.form.height > FREE_MAX_PIXELS;

  return (
    <section class="wb-pane" role="tabpanel" hidden={props.hidden} aria-label="出图">
      <aside class="wb-side">
        {modes.length > 1 && (
          <FieldBlock
            label="模式"
            hint="图生图 / 局部重绘按你的 Key 能力开放；原图与蒙版合计请留在 12 MiB 内。"
          >
            <div class="wb-modes">
              {modes.map((m) => (
                <button
                  key={m}
                  type="button"
                  class={cx("wb-mode", draw.mode === m && "wb-mode--on")}
                  onClick={() => patch({ mode: m })}
                >
                  {MODE_LABELS[m]}
                </button>
              ))}
            </div>
          </FieldBlock>
        )}
        <Field label="模型">
          <select class="select" value={draw.form.model} onChange={(e) => chooseModel(e.currentTarget.value)}>
            {models.map((m) => (
              <option key={m.id} value={m.id}>
                {m.label}
              </option>
            ))}
          </select>
        </Field>
        <div class="wb-duo">
          <Field label="质量预设" hint="自动拼在提示词末尾，不改动你写的字。">
            <select
              class="select"
              value={draw.qualityPreset}
              onChange={(e) => patch({ qualityPreset: e.currentTarget.value as QualityPreset })}
            >
              {qualityPresetList(draw.form.model).map((q) => (
                <option key={q} value={q}>
                  {q === "none" ? "无" : q === "standard" ? "标准" : "轻量（V5）"}
                </option>
              ))}
            </select>
          </Field>
          <Field label="负面预设">
            <select
              class="select"
              value={draw.ucPreset}
              onChange={(e) => patch({ ucPreset: e.currentTarget.value as UcPreset })}
            >
              {ucPresetList(draw.form.model).map((u) => (
                <option key={u} value={u}>
                  {UC_LABELS[u]}
                </option>
              ))}
            </select>
          </Field>
        </div>
        <Field label="正面提示词" hint="英文标签、逗号分隔；中文会由网关转译，但英文最稳。">
          <textarea
            class="textarea"
            rows={4}
            value={draw.form.positive}
            onInput={(e) => patchForm({ positive: e.currentTarget.value })}
            placeholder="1girl, seaside, sunset, very aesthetic"
          />
          <span class="wb-meter muted small">
            约 {estimateTokens(draw.form.positive)} / {tokenLimit(draw.form.model)} tokens（粗估）
          </span>
        </Field>
        {tagsOpen && (
          <div class="wb-suggest">
            <Button variant="ghost" size="sm" onClick={() => void suggestTags()} disabled={draw.suggest.busy}>
              {draw.suggest.busy ? "补全中…" : "标签补全"}
            </Button>
            {draw.suggest.tags.length > 0 && (
              <div class="wb-chips">
                {draw.suggest.tags.map((t) => (
                  <button key={t} type="button" class="wb-chip" onClick={() => appendTag(t)}>
                    + {t}
                  </button>
                ))}
              </div>
            )}
          </div>
        )}
        <Field label="画师词（可选）" hint="拼在提示词最前面；也可以直接写进提示词。">
          <input
            class="input"
            value={draw.artist}
            onInput={(e) => patch({ artist: e.currentTarget.value })}
          />
        </Field>
        <Field label="负面提示词" hint="想避免的元素；预设会拼在你的词前面。">
          <textarea
            class="textarea"
            rows={3}
            value={draw.form.negative}
            onInput={(e) => patchForm({ negative: e.currentTarget.value })}
            placeholder="lowres, bad hands"
          />
          <span class="wb-meter muted small">
            约 {estimateTokens(draw.form.negative)} / {tokenLimit(draw.form.model)} tokens（粗估）
          </span>
        </Field>
        {vibeOpen && (
          <FieldBlock
            label="参考图（Vibe）"
            hint={`最多 ${MAX_REFS} 张；强度越高越像原图。可能产生 Anlas 消耗，以请求记录为准。`}
          >
            <input
              type="file"
              accept="image/*"
              multiple
              onChange={(e) => void pickRefs(e.currentTarget.files)}
              aria-label="上传参考图"
            />
            {draw.refs.length > 0 && (
              <ul class="wb-refs">
                {draw.refs.map((r) => (
                  <li key={r.id}>
                    <img src={r.dataUrl} alt={`参考图 ${r.name}`} />
                    <label class="wb-ref__strength">
                      强度 {r.strength.toFixed(2)}
                      <input
                        type="range"
                        min={0}
                        max={1}
                        step={0.05}
                        value={r.strength}
                        onInput={(e) =>
                          patch({
                            refs: draw.refs.map((x) =>
                              x.id === r.id ? { ...x, strength: Number(e.currentTarget.value) } : x,
                            ),
                          })
                        }
                      />
                    </label>
                    <Button
                      variant="ghost"
                      size="sm"
                      onClick={() => patch({ refs: draw.refs.filter((x) => x.id !== r.id) })}
                    >
                      移除
                    </Button>
                  </li>
                ))}
              </ul>
            )}
          </FieldBlock>
        )}
        {draw.mode !== "generate" && (
          <FieldBlock
            label={draw.mode === "infill" ? "原图与蒙版" : "原图"}
            hint={
              draw.mode === "infill" ? "蒙版：白色 = 要重画的地方，黑色 = 保留。" : "强度越低越接近原图。"
            }
          >
            <input
              type="file"
              accept="image/*"
              onChange={(e) => void pickSource(e.currentTarget.files)}
              aria-label="上传原图"
            />
            {draw.source && (
              <p class="small">
                原图：{draw.source.name}（
                <button type="button" class="wb-link" onClick={() => patch({ source: null })}>
                  移除
                </button>
                ）
              </p>
            )}
            {draw.mode === "infill" && (
              <>
                <input
                  type="file"
                  accept="image/*"
                  onChange={(e) => void pickMask(e.currentTarget.files)}
                  aria-label="上传蒙版"
                />
                {draw.mask && (
                  <p class="small">
                    蒙版：{draw.mask.name}（
                    <button type="button" class="wb-link" onClick={() => patch({ mask: null })}>
                      移除
                    </button>
                    ）
                  </p>
                )}
              </>
            )}
            <label class="wb-range">
              强度 {draw.strength.toFixed(2)}
              <input
                type="range"
                min={0}
                max={1}
                step={0.05}
                value={draw.strength}
                onInput={(e) => patch({ strength: Number(e.currentTarget.value) })}
              />
            </label>
            {draw.mode === "img2img" && (
              <label class="wb-range">
                噪声 {draw.noise.toFixed(2)}
                <input
                  type="range"
                  min={0}
                  max={1}
                  step={0.05}
                  value={draw.noise}
                  onInput={(e) => patch({ noise: Number(e.currentTarget.value) })}
                />
              </label>
            )}
          </FieldBlock>
        )}
        <FieldBlock
          label="尺寸"
          hint={overPixels ? "超出免费规格（1024×1024），送出时会等比缩小并说明。" : undefined}
        >
          <select
            class="select"
            value={draw.form.preset}
            onChange={(e) => choosePreset(e.currentTarget.value)}
            aria-label="尺寸预设"
          >
            {SIZE_PRESETS.map((p) => (
              <option key={p.id} value={p.id}>
                {p.label}
              </option>
            ))}
          </select>
          {draw.form.preset === "custom" && (
            <div class="wb-duo">
              <input
                class="input"
                type="number"
                min={64}
                max={4096}
                step={64}
                value={draw.form.width}
                onInput={(e) => patchForm({ width: Number(e.currentTarget.value) })}
                aria-label="宽度"
              />
              <input
                class="input"
                type="number"
                min={64}
                max={4096}
                step={64}
                value={draw.form.height}
                onInput={(e) => patchForm({ height: Number(e.currentTarget.value) })}
                aria-label="高度"
              />
            </div>
          )}
        </FieldBlock>
        <div class="wb-duo">
          <Field label="步数" hint="免费档最多 28 步。">
            <input
              class="input"
              type="number"
              min={1}
              max={50}
              value={draw.form.steps}
              onInput={(e) => patchForm({ steps: Number(e.currentTarget.value) })}
            />
          </Field>
          <Field label="引导（Scale）">
            <input
              class="input"
              type="number"
              min={0}
              max={10}
              step={0.5}
              value={draw.form.scale}
              onInput={(e) => patchForm({ scale: Number(e.currentTarget.value) })}
            />
          </Field>
        </div>
        <div class="wb-duo">
          <Field label="采样器">
            <select
              class="select"
              value={draw.form.sampler}
              onChange={(e) => patchForm({ sampler: e.currentTarget.value })}
            >
              {samplers.map((s) => (
                <option key={s} value={s}>
                  {s}
                </option>
              ))}
            </select>
          </Field>
          {hasSchedule(draw.form.model) && (
            <Field label="噪声调度">
              <select
                class="select"
                value={draw.form.noise}
                onChange={(e) => patchForm({ noise: e.currentTarget.value })}
              >
                {NOISE_SCHEDULES.map((n) => (
                  <option key={n} value={n}>
                    {n}
                  </option>
                ))}
              </select>
            </Field>
          )}
        </div>
        {hasSchedule(draw.form.model) && (
          <Field label="CFG Rescale" hint="0 = 不动；拉高画面更「平」，一般 0~0.5。">
            <input
              class="input"
              type="number"
              min={0}
              max={1}
              step={0.05}
              value={draw.form.cfgRescale}
              onInput={(e) => patchForm({ cfgRescale: Number(e.currentTarget.value) })}
            />
          </Field>
        )}
        {isV3(draw.form.model) && (
          <label class="wb-check">
            <input
              type="checkbox"
              checked={draw.form.smea}
              onChange={(e) => patchForm({ smea: e.currentTarget.checked })}
            />
            SMEA（V3 动态，出图更慢）
          </label>
        )}
        <Field label="种子" hint="-1 或留空 = 随机；想复现一张图就把它的种子填回来。">
          <input
            class="input"
            value={draw.form.seed}
            onInput={(e) => patchForm({ seed: e.currentTarget.value })}
            placeholder="-1"
          />
        </Field>
        <div class="wb-submit">
          <p class="muted small">
            {costHint(draw.form.model, paid)}；免费档最多 {FREE_MAX_STEPS} 步、单张 ≤ 1024×1024。
          </p>
          <Button variant="primary" size="lg" onClick={() => void generate()} disabled={running}>
            生成
          </Button>
        </div>
      </aside>
      <section class="wb-stage" aria-label="出图结果">
        {error && <Notice tone="danger">{error}</Notice>}
        {note && <Notice tone="warn">{note}</Notice>}
        {running ? (
          <div class="wb-wait">
            <p>正在出图… 已等 {elapsed} 秒</p>
            <p class="muted small">排队 + 生成一般几十秒；上游忙时会更久。取消等待不一定能省下这次额度。</p>
            <Button variant="ghost" onClick={() => abort.current?.abort()}>
              取消等待
            </Button>
          </div>
        ) : shot ? (
          <div class="wb-result">
            <button type="button" class="wb-shot" onClick={() => props.openShot(shot)}>
              <img src={shot.url} alt={`成图 ${shot.attrs.width}×${shot.attrs.height}`} />
            </button>
            <div class="wb-shot__actions">
              <a class="btn btn--sm" href={shot.url} download={`owl-${shot.attrs.seed}.${shot.ext}`}>
                下载
              </a>
              <Button variant="ghost" size="sm" onClick={() => props.openShot(shot)}>
                大图与参数
              </Button>
              <Button variant="ghost" size="sm" onClick={() => patchForm({ seed: String(shot.attrs.seed) })}>
                用这颗种子再画
              </Button>
            </div>
          </div>
        ) : (
          <div class="wb-empty">
            <p>写好提示词，点「生成」。</p>
            <p class="muted small">
              成图会出现在这里，也会进右边的「本次画廊」。免费档会自动钳到免费规格并在上方说明；付费 Key
              超出规格的部分按 Anlas 计费。
            </p>
          </div>
        )}
      </section>
    </section>
  );
}

/* ---------------- 画廊 ---------------- */

function HistoryRail({ shots, onOpen }: { shots: Shot[]; onOpen: (s: Shot) => void }) {
  return (
    <aside class="wb-rail" aria-label="本次画廊">
      <h2>本次画廊</h2>
      {shots.length === 0 ? (
        <p class="muted small">出过的图都收在这里（最多 20 张），刷新页面就清空。想保留就点开下载。</p>
      ) : (
        <div class="wb-thumbs">
          {shots.map((s, i) => (
            <button key={`${s.attrs.seed}-${i}`} type="button" class="wb-thumb" onClick={() => onOpen(s)}>
              <img src={s.url} alt={`画廊图 ${s.attrs.seed}`} />
            </button>
          ))}
        </div>
      )}
    </aside>
  );
}

function ShotDialog({
  shot,
  onClose,
  onReuse,
}: {
  shot: Shot | null;
  onClose: () => void;
  onReuse: (seed: number) => void;
}) {
  return (
    <Dialog
      open={shot !== null}
      title="成图详情"
      onClose={onClose}
      wide
      footer={
        <>
          {shot && (
            <>
              <a class="btn" href={shot.url} download={`owl-${shot.attrs.seed}.${shot.ext}`}>
                下载
              </a>
              <Button variant="ghost" onClick={() => onReuse(shot.attrs.seed)}>
                用这颗种子再画
              </Button>
            </>
          )}
          <Button variant="ghost" onClick={onClose}>
            关闭
          </Button>
        </>
      }
    >
      {shot && (
        <div class="wb-dialog">
          <img src={shot.url} alt={`成图 ${shot.attrs.seed}`} />
          <dl class="wb-attrs">
            <div>
              <dt>模型</dt>
              <dd>{modelOf(shot.attrs.model).label}</dd>
            </div>
            <div>
              <dt>尺寸</dt>
              <dd>
                {shot.attrs.width}×{shot.attrs.height}
              </dd>
            </div>
            <div>
              <dt>步数 / 引导</dt>
              <dd>
                {shot.attrs.steps} / {shot.attrs.scale}
              </dd>
            </div>
            <div>
              <dt>采样器</dt>
              <dd>{shot.attrs.sampler}</dd>
            </div>
            <div>
              <dt>种子</dt>
              <dd>{shot.attrs.seed}</dd>
            </div>
            <div>
              <dt>耗时</dt>
              <dd>{shot.attrs.seconds.toFixed(1)} 秒</dd>
            </div>
            <div class="wb-attr--wide">
              <dt>提示词</dt>
              <dd>{shot.attrs.prompt}</dd>
            </div>
            {shot.attrs.negative && (
              <div class="wb-attr--wide">
                <dt>负面</dt>
                <dd>{shot.attrs.negative}</dd>
              </div>
            )}
          </dl>
        </div>
      )}
    </Dialog>
  );
}

/* ---------------- 小工具 ---------------- */

function displayName(me: MeMember): string {
  return me.discord?.display_name ?? me.discord?.username ?? "成员";
}

/** 与画匣同款的入口闸门（lib/nai-config），这里只是转一道。 */
function entryProblem(me: MeMember): string | null {
  if (!me.key) return "还没有拿到这把 Key 的完整信息，刷新页面后再试。";
  if (me.requires_web_login) return "先用 Discord 在页面里登录一次，登录后才能出图。";
  if (me.policy?.image === false) return "出图功能没有开放给你的 Key，如有疑问请在 Discord 联系站长。";
  return null;
}
