/**
 * 画匣（Beta）：成员首页的网页出图工坊。仿「魔女画匣」的用法重写：
 * 参数面板（模型 / 尺寸 / 步数 / 引导 / 采样器 / 种子）、串行生成、成图预览与下载、本次会话的历史。
 * 接的是网关自己的出图接口（/ai/generate-image，Bearer = 成员自己的 Key），不引入第三方依赖。
 * 风险提示都写在这里：出图计入今日额度、取消不一定免费、历史只留在本页。
 */

import type { ComponentChildren } from "preact";
import { useEffect, useRef, useState } from "preact/hooks";
import { Badge, Button, Card, cx, Dialog, Field, Notice, toast } from "../../ui";
import { useStore } from "./ctx";
import {
  BASE_MODELS,
  buildRequest,
  clampForm,
  costHint,
  DEFAULT_FORM,
  entryProblem,
  FREE_MAX_PIXELS,
  FREE_MAX_STEPS,
  type GenAttrs,
  isV3,
  isV5,
  NOISE_SCHEDULES,
  type PaintForm,
  parseSuggestions,
  promptProblem,
  resolveSeed,
  SIZE_PRESETS,
  samplersFor,
  V5_MODELS,
} from "./paintbox-config";
import { extractImages, type ImageBytes, imageExt, toDataUrl } from "./paintbox-image";
import { modelLabel } from "./Requests";
import type { MeMember } from "./types";
import "./paintbox.css";

type Shot = { url: string; attrs: GenAttrs; ext: string };

/** 出图接口的错误体与官方客户端一致：{statusCode, message, error:{message, status, request_id}}。 */
async function genError(res: Response): Promise<Error> {
  let msg = `请求失败（HTTP ${res.status}）`;
  try {
    const b = (await res.json()) as {
      message?: string;
      error?: { message?: string; request_id?: string };
    } | null;
    const m = b?.error?.message ?? b?.message;
    if (m) msg = m.replace(/^猫头鹰公益站提醒：/, "");
  } catch {
    /* 错误体不是 JSON：用默认文案 */
  }
  return new Error(msg);
}

async function postGenerate(body: unknown, key: string, signal: AbortSignal): Promise<ImageBytes[]> {
  const res = await fetch("/ai/generate-image", {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      Accept: "binary/octet-stream, application/json",
      Authorization: `Bearer ${key}`,
    },
    body: JSON.stringify(body),
    signal,
  });
  if (!res.ok) throw await genError(res);
  return extractImages(new Uint8Array(await res.arrayBuffer()));
}

async function fetchSuggestions(prompt: string, model: string, key: string): Promise<string[]> {
  const url = `/ai/generate-image/suggest-tags?prompt=${encodeURIComponent(prompt)}&model=${encodeURIComponent(model)}`;
  const res = await fetch(url, { headers: { Accept: "application/json", Authorization: `Bearer ${key}` } });
  if (!res.ok) throw await genError(res);
  return parseSuggestions(await res.json());
}

function AttrRow(props: { label: string; wide?: boolean; children: ComponentChildren }) {
  return (
    <div class={cx("pb-attr", props.wide && "pb-attr--wide")}>
      <dt>{props.label}</dt>
      <dd>{props.children}</dd>
    </div>
  );
}

/** 成图卡（预览大图和历史缩略图共用）：图可点开大图，动作固定为下载 / 用这颗种子。 */
function ShotView(props: { shot: Shot; onSeed: (seed: number) => void; onOpen: () => void }) {
  const { shot } = props;
  return (
    <div class="pb-shot">
      <button type="button" class="pb-shot__img" onClick={props.onOpen}>
        <img src={shot.url} alt="这次生成的图片，点开看大图" />
      </button>
      <div class="pb-shot__actions">
        <a class="btn" href={shot.url} download={`owl-${shot.attrs.seed}.${shot.ext}`}>
          下载
        </a>
        <Button onClick={() => props.onSeed(shot.attrs.seed)}>用这颗种子再画</Button>
      </div>
    </div>
  );
}

export function Paintbox({ me }: { me: MeMember }) {
  // 队列节奏与额度刷新走首页同一份数据层：live 每秒一跳（只读 cycle），生成成功后刷一次 me（圆环会重新充满）
  const { live, reloadMe } = useStore();
  const paid = me.policy?.paid_parameters !== false;
  const v5Open = me.policy?.v5 === true;
  const tagsOpen = me.policy?.tags === true;
  const blocked = entryProblem(me);

  const [form, setForm] = useState<PaintForm>(() => ({ ...DEFAULT_FORM }));
  const set = (patch: Partial<PaintForm>) => setForm((f) => ({ ...f, ...patch }));
  const [running, setRunning] = useState(false);
  const [elapsed, setElapsed] = useState(0);
  const [note, setNote] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [shot, setShot] = useState<Shot | null>(null);
  const [history, setHistory] = useState<Shot[]>([]);
  const [viewing, setViewing] = useState<Shot | null>(null);
  const [suggest, setSuggest] = useState<{ busy: boolean; tags: string[] }>({ busy: false, tags: [] });
  const abort = useRef<AbortController | null>(null);

  // 生成中的秒表：只在 running 时走，每秒一格
  useEffect(() => {
    if (!running) return;
    setElapsed(0);
    const t = setInterval(() => setElapsed((s) => s + 1), 1000);
    return () => clearInterval(t);
  }, [running]);

  const choosePreset = (id: string) => {
    const p = SIZE_PRESETS.find((x) => x.id === id);
    set(p?.w && p?.h ? { preset: id, width: p.w, height: p.h } : { preset: id });
  };
  const chooseModel = (id: string) => {
    const list = samplersFor(id);
    const keep = list.includes(form.sampler) ? form.sampler : (list[0] ?? "k_euler_ancestral");
    set({ model: id, sampler: keep, smea: isV3(id) ? form.smea : false });
  };

  const generate = async () => {
    if (running || blocked) return;
    const problem = promptProblem(form);
    if (problem) {
      setError(problem);
      return;
    }
    setError(null);
    const { form: send, notes } = clampForm(form, paid);
    const seed = resolveSeed(send.seed);
    const body = buildRequest(send, seed);
    setNote(notes.length ? notes.join("；") : null);
    setRunning(true);
    const controller = new AbortController();
    abort.current = controller;
    const startedAt = Date.now();
    try {
      const imgs = await postGenerate(body, me.key ?? "", controller.signal);
      const first = imgs[0];
      if (!first) throw new Error("返回里没有图片。");
      const next: Shot = {
        url: toDataUrl(first),
        ext: imageExt(first.mime),
        attrs: {
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
        },
      };
      setShot(next);
      setHistory((h) => [next, ...h].slice(0, 10));
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

  const suggestTags = async () => {
    if (suggest.busy || !me.key) return;
    const prompt = form.positive.trim();
    if (!prompt) {
      toast("先写几个开头（人物、场景都行），再点补全。");
      return;
    }
    setSuggest({ busy: true, tags: [] });
    try {
      const tags = await fetchSuggestions(prompt, form.model, me.key);
      if (!tags.length) toast("这次没拿到标签建议，换个写法再试。");
      else setSuggest({ busy: false, tags });
    } catch (e) {
      toast(e instanceof Error ? e.message : "标签补全没有成功，请稍后再试。", { error: true });
      setSuggest({ busy: false, tags: [] });
    }
  };

  const appendTag = (tag: string) => {
    const cur = form.positive.trim();
    if (new RegExp(`(^|,\\s*)${tag.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}(\\s*,|$)`).test(cur)) return;
    set({ positive: cur ? `${cur}, ${tag}` : tag });
  };

  const overPixels = !paid && form.width * form.height > FREE_MAX_PIXELS;
  const cycle = live.value?.cycle;
  const cycleHint = cycle && cycle > 0 ? Math.round(cycle) : null;

  return (
    // 不加 reveal（同「我的请求」）：显影按「整块滚进来多少」算，画匣整卡太高，滚很久都还是糊的
    <Card
      id="paintbox"
      title={
        <>
          画匣 <Badge tone="accent">Beta</Badge>
        </>
      }
      sub="网页里直接出图，不用配置客户端。测试期间界面和参数还会调整，欢迎在 Discord 提意见。"
    >
      {blocked ? (
        <Notice tone="warn">{blocked}</Notice>
      ) : (
        <div class="pb" aria-busy={running ? "true" : undefined}>
          <Field
            label="提示词"
            hint="英文标签、逗号分隔；权重写 (tag:1.2) 也可以，网关会自动换算。本站不自动补质量词，可在开头加 very aesthetic, masterpiece 这类词。"
          >
            <textarea
              class="textarea"
              rows={3}
              value={form.positive}
              placeholder="very aesthetic, masterpiece, 1girl, sunset, seaside"
              onInput={(e) => set({ positive: (e.target as HTMLTextAreaElement).value })}
            />
          </Field>
          {tagsOpen && (
            <div class="pb-suggest">
              <Button size="sm" onClick={() => void suggestTags()} disabled={suggest.busy}>
                {suggest.busy ? "补全中…" : "按这段补全标签"}
              </Button>
              {suggest.tags.length > 0 && (
                <span class="pb-suggest__tags">
                  {suggest.tags.map((t) => (
                    <button
                      key={t}
                      type="button"
                      class="pb-chip"
                      onClick={() => appendTag(t)}
                      title="点一下加进提示词"
                    >
                      {t}
                    </button>
                  ))}
                </span>
              )}
            </div>
          )}
          <Field label="负面提示词（可选）" hint="不想要的内容，比如 lowres, bad anatomy。留空也可以。">
            <textarea
              class="textarea"
              rows={2}
              value={form.negative}
              placeholder="lowres, bad anatomy"
              onInput={(e) => set({ negative: (e.target as HTMLTextAreaElement).value })}
            />
          </Field>

          <div class="pb-grid">
            <Field
              label="模型"
              hint={isV5(form.model) ? "V5 计入今天的 V5 额度，站方可临时降档为 V5 Medium。" : undefined}
            >
              <select
                class="select"
                value={form.model}
                onChange={(e) => chooseModel((e.target as HTMLSelectElement).value)}
                disabled={!!running}
              >
                {BASE_MODELS.map((m) => (
                  <option key={m.id} value={m.id}>
                    {m.label}
                  </option>
                ))}
                {v5Open && (
                  <optgroup label="需要 V5 额度">
                    {V5_MODELS.map((m) => (
                      <option key={m.id} value={m.id}>
                        {m.label}
                      </option>
                    ))}
                  </optgroup>
                )}
              </select>
            </Field>
            <Field
              label="尺寸"
              hint={overPixels ? "超出免费规格（1024×1024），送出时会自动缩小。" : undefined}
            >
              <select
                class="select"
                value={form.preset}
                onChange={(e) => choosePreset((e.target as HTMLSelectElement).value)}
                disabled={!!running}
              >
                {SIZE_PRESETS.map((p) => (
                  <option key={p.id} value={p.id}>
                    {p.label}
                  </option>
                ))}
              </select>
            </Field>
            {form.preset === "custom" && (
              <>
                <Field label="宽">
                  <input
                    class="input"
                    type="number"
                    min={64}
                    max={4096}
                    step={64}
                    value={form.width}
                    onInput={(e) => set({ width: Number((e.target as HTMLInputElement).value) || 0 })}
                    disabled={!!running}
                  />
                </Field>
                <Field label="高">
                  <input
                    class="input"
                    type="number"
                    min={64}
                    max={4096}
                    step={64}
                    value={form.height}
                    onInput={(e) => set({ height: Number((e.target as HTMLInputElement).value) || 0 })}
                    disabled={!!running}
                  />
                </Field>
              </>
            )}
            <Field
              label="步数"
              hint={
                !paid && form.steps > FREE_MAX_STEPS
                  ? `免费规格最多 ${FREE_MAX_STEPS} 步。`
                  : `1–50，越大越细也越慢；免费规格最多 ${FREE_MAX_STEPS}。`
              }
            >
              <input
                class="input"
                type="number"
                min={1}
                max={50}
                value={form.steps}
                onInput={(e) => set({ steps: Number((e.target as HTMLInputElement).value) || 0 })}
                disabled={!!running}
              />
            </Field>
            <Field label="引导" hint="0–10，越高越贴提示词，太高画面会僵。">
              <input
                class="input"
                type="number"
                min={0}
                max={10}
                step={0.5}
                value={form.scale}
                onInput={(e) => set({ scale: Number((e.target as HTMLInputElement).value) || 0 })}
                disabled={!!running}
              />
            </Field>
            <Field label="采样器">
              <select
                class="select"
                value={form.sampler}
                onChange={(e) => set({ sampler: (e.target as HTMLSelectElement).value })}
                disabled={!!running}
              >
                {samplersFor(form.model).map((s) => (
                  <option key={s} value={s}>
                    {s}
                  </option>
                ))}
              </select>
            </Field>
            <Field label="种子" hint="-1 = 每次随机；固定种子 + 相同参数可以复现。">
              <input
                class="input"
                type="text"
                inputMode="numeric"
                value={form.seed}
                placeholder="-1"
                onInput={(e) => set({ seed: (e.target as HTMLInputElement).value })}
                disabled={!!running}
              />
            </Field>
            {!isV3(form.model) && (
              <Field label="噪声调度">
                <select
                  class="select"
                  value={form.noise}
                  onChange={(e) => set({ noise: (e.target as HTMLSelectElement).value })}
                  disabled={!!running}
                >
                  {NOISE_SCHEDULES.map((s) => (
                    <option key={s} value={s}>
                      {s}
                    </option>
                  ))}
                </select>
              </Field>
            )}
            {isV3(form.model) && (
              <Field label="SMEA" hint="V3 专属开关，V4 系没有这个选项。">
                <span class="pb-check">
                  <input
                    type="checkbox"
                    checked={form.smea}
                    onChange={(e) => set({ smea: (e.target as HTMLInputElement).checked })}
                    disabled={!!running}
                  />
                  开启
                </span>
              </Field>
            )}
          </div>

          <div class="pb-actions">
            <Button variant="primary" size="lg" onClick={() => void generate()} disabled={!!running}>
              {running ? "画一张中…" : "生成"}
            </Button>
            {running && (
              <>
                <Button
                  variant="ghost"
                  onClick={() => abort.current?.abort()}
                  title="排队阶段取消不扣额度；已经开始画的会照常计入。"
                >
                  取消等待
                </Button>
                <span class="pb-wait" role="status">
                  已等 {elapsed} 秒{cycleHint ? ` · 全站平均每张约 ${cycleHint} 秒` : ""}
                </span>
              </>
            )}
            {!running && <span class="pb-cost muted small">{costHint(form.model, paid)}</span>}
          </div>

          {note && <Notice tone="warn">{note}</Notice>}
          {error && <Notice tone="danger">{error}</Notice>}

          <div class="pb-result" aria-live="polite">
            {shot && !running && (
              <ShotView
                shot={shot}
                onSeed={(s) => set({ seed: String(s) })}
                onOpen={() => setViewing(shot)}
              />
            )}
            {!shot && !running && !error && (
              <p class="muted pb-empty">画好的图会出现在这里。第一次用：写好提示词，点「生成」就行。</p>
            )}
            {history.length > 1 && (
              <div class="pb-history">
                <p class="muted small">这次的会话里画过（刷新页面就清空，喜欢的先下载）：</p>
                <div class="pb-thumbs">
                  {history.map((h) => (
                    <button
                      key={h.attrs.seed + String(h.attrs.seconds)}
                      type="button"
                      class="pb-thumb"
                      onClick={() => setViewing(h)}
                      aria-label={`查看 ${h.attrs.prompt.slice(0, 20)} 的成图`}
                    >
                      <img src={h.url} alt="" />
                    </button>
                  ))}
                </div>
              </div>
            )}
          </div>
        </div>
      )}

      <Dialog open={viewing !== null} title="成图" onClose={() => setViewing(null)} wide>
        {viewing && (
          <div class="pb-view">
            <img src={viewing.url} alt="生成的图片" />
            <dl class="pb-attrs">
              <AttrRow label="模型">{modelLabel({ model: viewing.attrs.model, kind: "image" })}</AttrRow>
              <AttrRow label="尺寸">{`${viewing.attrs.width}×${viewing.attrs.height} · ${viewing.attrs.steps} 步`}</AttrRow>
              <AttrRow label="引导 / 采样">
                {`${viewing.attrs.scale} · ${viewing.attrs.sampler}`}
                {!isV3(viewing.attrs.model) ? ` · ${viewing.attrs.noise}` : ""}
              </AttrRow>
              <AttrRow label="种子">
                <button
                  type="button"
                  class="pb-seedlink"
                  onClick={() => set({ seed: String(viewing.attrs.seed) })}
                  title="填回种子栏"
                >
                  {viewing.attrs.seed}
                </button>
              </AttrRow>
              <AttrRow label="耗时">{`${Math.round(viewing.attrs.seconds)} 秒`}</AttrRow>
              <AttrRow label="提示词" wide>
                {viewing.attrs.prompt}
              </AttrRow>
              {viewing.attrs.negative && (
                <AttrRow label="负面" wide>
                  {viewing.attrs.negative}
                </AttrRow>
              )}
            </dl>
            <div class="pb-shot__actions">
              <a class="btn" href={viewing.url} download={`owl-${viewing.attrs.seed}.${viewing.ext}`}>
                下载
              </a>
              <Button
                onClick={() => {
                  set({ seed: String(viewing.attrs.seed) });
                  setViewing(null);
                }}
              >
                用这颗种子再画
              </Button>
            </div>
          </div>
        )}
      </Dialog>
    </Card>
  );
}
