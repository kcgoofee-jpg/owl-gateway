/**
 * 批量出图：五组提示词（nai-workbench 的「多组提示词批量」口径）——
 * 画师 / 质量 / 负面三个共用词，每组可单独改写（unified 开关关掉时用组内的）；
 * 每组张数 1..20，一次总量封顶 20。队列在浏览器里按顺序一张一张跑：
 * 限速（429 且明确没开始画）按 Retry-After 自动等；失败或「结果待核对」立即暂停整条队列等人决定。
 * 页面关掉队列就没了；没送出的任务不会计费。
 */
import { useRef, useState } from "preact/hooks";
import { clampForm, type PaintForm, resolveSeed } from "../../lib/nai-config";
import type { ImageBytes } from "../../lib/nai-image";
import { cx } from "../../ui";
import { makeShot, postImages, type Shot, WorkbenchError } from "./api";
import { reloadMe } from "./store";
import {
  batchTasks,
  buildTaskRequest,
  clampCount,
  clampRetryAfter,
  classifyFailure,
  MAX_BATCH_TASKS,
  MAX_PER_GROUP,
  type QualityPreset,
  type Strings,
  type UcPreset,
  type WbMode,
  type WbTask,
} from "./workbench-config";

type JobStatus = "queued" | "running" | "done" | "failed" | "unknown" | "cancelled";
type Job = { status: JobStatus; detail?: string | undefined; seconds?: number | undefined };

const STATUS_LABELS: Record<JobStatus, string> = {
  queued: "排队中",
  running: "出图中…",
  done: "已完成",
  failed: "失败",
  unknown: "结果待核对",
  cancelled: "已取消",
};

const UNIFIED_LABELS = { artist: "画师词用共用", quality: "质量词用共用", negative: "负面词用共用" } as const;

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/** 读当前队列阶段（过一道函数，避免 TS 把 ref 的赋值历史当成常量收窄）。 */
const phaseNow = (ref: { current: "idle" | "running" | "paused" }) => ref.current;

export function BatchPane(props: {
  hidden: boolean;
  base: PaintForm;
  presets: { qualityPreset: QualityPreset; ucPreset: UcPreset };
  mode: WbMode;
  paid: boolean;
  apiKey: string;
  shared: Strings;
  setShared: (s: Strings) => void;
  unified: Record<"artist" | "quality" | "negative", boolean>;
  setUnified: (u: Record<"artist" | "quality" | "negative", boolean>) => void;
  groups: import("./workbench-config").BatchGroup[];
  setGroups: (g: import("./workbench-config").BatchGroup[]) => void;
  addShots: (shots: Shot[]) => void;
  openShot: (s: Shot) => void;
}) {
  const { base, presets, paid, shared, unified, groups } = props;
  const [phase, setPhase] = useState<"idle" | "running" | "paused">("idle");
  const [jobs, setJobs] = useState<Job[]>([]);
  // 队列循环读 ref：暂停 / 继续不用重启循环；开跑时的任务快照也存这里，展示与实际送出永远一致
  const phaseRef = useRef<"idle" | "running" | "paused">("idle");
  const frozen = useRef<WbTask[]>([]);
  const shots = useRef<(Shot | null)[]>([]);
  const abortRef = useRef<AbortController | null>(null);

  const tasks = batchTasks(base, presets, shared, unified, groups);

  const setJob = (i: number, patch: Partial<Job>) =>
    setJobs((js) => js.map((j, k) => (k === i ? { ...j, ...patch } : j)));

  const start = async () => {
    if (phaseRef.current !== "idle" || !tasks.length) return;
    frozen.current = tasks;
    shots.current = tasks.map(() => null);
    setJobs(tasks.map(() => ({ status: "queued" as JobStatus })));
    phaseRef.current = "running";
    setPhase("running");
    for (let i = 0; i < tasks.length; i++) {
      // 暂停就停在任务边界；恢复后从这里接着跑
      while (phaseNow(phaseRef) === "paused") await sleep(400);
      if (phaseNow(phaseRef) === "idle") break;
      const task = tasks[i];
      if (!task) break;
      setJob(i, { status: "running" });
      const { form: send, notes } = clampForm(task.form, paid);
      const seed = resolveSeed(send.seed);
      const body = buildTaskRequest({ form: send, mode: "generate", strength: 0, noise: 0, refs: [] }, seed);
      const controller = new AbortController();
      abortRef.current = controller;
      const startedAt = Date.now();
      try {
        let imgs: ImageBytes[] | null = null;
        // 限速重试：429 且明确没开始画才等；最多等 3 次，之后这组按失败处理
        for (let attempt = 0; attempt < 3; attempt++) {
          try {
            imgs = await postImages(body, props.apiKey, controller.signal);
            break;
          } catch (e) {
            if (e instanceof DOMException && e.name === "AbortError") throw e;
            const canRetry = e instanceof WorkbenchError && classifyFailure(e.status, e) === null;
            if (!canRetry || attempt === 2) throw e;
            const wait = clampRetryAfter(e instanceof WorkbenchError ? e.retryAfter : null);
            setJob(i, { detail: `限速中，${wait} 秒后自动重试（第 ${attempt + 1} 次）` });
            await sleep(wait * 1000);
            if (phaseNow(phaseRef) === "idle") throw new DOMException("cancelled", "AbortError");
          }
        }
        const first = imgs?.[0];
        if (!first) throw new Error("返回里没有图片。");
        const attrs = {
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
        };
        const shot = makeShot(first, attrs, task.label);
        shots.current[i] = shot;
        props.addShots([shot]);
        setJob(i, {
          status: "done",
          seconds: attrs.seconds,
          detail: notes.length ? notes.join("；") : undefined,
        });
        void reloadMe();
      } catch (e) {
        if (e instanceof DOMException && e.name === "AbortError") {
          setJob(i, {
            status: "cancelled",
            detail: "已取消。如果网关已经开始画，这次仍可能计入今天的额度。",
          });
        } else {
          const f = failureOf(e);
          setJob(i, { status: f.status, detail: f.message });
        }
        // 失败 / 取消都暂停整条队列：剩下的还在浏览器里，等人决定
        phaseRef.current = "paused";
        setPhase("paused");
      } finally {
        abortRef.current = null;
      }
    }
    if (phaseRef.current === "running") {
      phaseRef.current = "idle";
      setPhase("idle");
    }
  };

  const pause = () => {
    phaseRef.current = "paused";
    setPhase("paused");
  };
  const resume = () => {
    phaseRef.current = "running";
    setPhase("running");
  };
  const stop = () => {
    // 清空重来：中止在途请求，循环下一拍自然退出
    phaseRef.current = "idle";
    setPhase("idle");
    abortRef.current?.abort();
  };
  const cancelCurrent = () => abortRef.current?.abort();

  const done = jobs.filter((j) => j.status === "done").length;
  const list = jobs.length ? frozen.current : [];

  return (
    <section class="nai-shell nai-batch" role="tabpanel" hidden={props.hidden} aria-label="批量出图">
      <aside class="nai-sidebar wb-side">
        <div class="nai-scroll">
          <p class="nai-note">
            批量按顺序一张一张跑。限速会自动等待，失败会暂停整条队列。批量只跑文生图，模型、尺寸和步数用绘图页的参数。
          </p>
          {props.mode !== "generate" && <p class="nai-note">绘图页当前不是文生图，批量仍按文生图送出。</p>}
          <div class="nai-card">
            <span class="nai-meter">共用词。开关打开时所有组都用这里的词。</span>
            <input
              class="nai-input"
              value={shared.artist}
              onInput={(e) => props.setShared({ ...shared, artist: e.currentTarget.value })}
              placeholder="共用画师词"
              aria-label="共用画师词"
            />
            <input
              class="nai-input"
              value={shared.quality}
              onInput={(e) => props.setShared({ ...shared, quality: e.currentTarget.value })}
              placeholder="共用质量词"
              aria-label="共用质量词"
            />
            <input
              class="nai-input"
              value={shared.negative}
              onInput={(e) => props.setShared({ ...shared, negative: e.currentTarget.value })}
              placeholder="共用负面词"
              aria-label="共用负面词"
            />
          </div>
          <div class="wb-unified">
            {(["artist", "quality", "negative"] as const).map((k) => (
              <label key={k} class="wb-check">
                <input
                  type="checkbox"
                  checked={unified[k]}
                  onChange={(e) => props.setUnified({ ...unified, [k]: e.currentTarget.checked })}
                />
                {UNIFIED_LABELS[k]}
              </label>
            ))}
          </div>
          {groups.map((g, i) => (
            <div class="nai-card" key={g.id}>
              <label class="wb-check">
                <input
                  type="checkbox"
                  checked={g.enabled}
                  onChange={(e) =>
                    props.setGroups(
                      groups.map((x, k) => (k === i ? { ...x, enabled: e.currentTarget.checked } : x)),
                    )
                  }
                />
                参与批量
              </label>
              <textarea
                class="nai-textarea"
                rows={2}
                value={g.prompt}
                onInput={(e) =>
                  props.setGroups(
                    groups.map((x, k) => (k === i ? { ...x, prompt: e.currentTarget.value } : x)),
                  )
                }
                placeholder="这组的提示词"
                aria-label={`第 ${i + 1} 组提示词`}
              />
              {!unified.artist && (
                <input
                  class="nai-input"
                  value={g.artist}
                  onInput={(e) =>
                    props.setGroups(
                      groups.map((x, k) => (k === i ? { ...x, artist: e.currentTarget.value } : x)),
                    )
                  }
                  placeholder="这组的画师词"
                  aria-label={`第 ${i + 1} 组画师词`}
                />
              )}
              {!unified.quality && (
                <input
                  class="nai-input"
                  value={g.quality}
                  onInput={(e) =>
                    props.setGroups(
                      groups.map((x, k) => (k === i ? { ...x, quality: e.currentTarget.value } : x)),
                    )
                  }
                  placeholder="这组的质量词"
                  aria-label={`第 ${i + 1} 组质量词`}
                />
              )}
              {!unified.negative && (
                <input
                  class="nai-input"
                  value={g.negative}
                  onInput={(e) =>
                    props.setGroups(
                      groups.map((x, k) => (k === i ? { ...x, negative: e.currentTarget.value } : x)),
                    )
                  }
                  placeholder="这组的负面词"
                  aria-label={`第 ${i + 1} 组负面词`}
                />
              )}
              <label class="wb-range">
                张数
                <input
                  class="nai-input wb-group__count"
                  type="number"
                  min={1}
                  max={20}
                  value={g.count}
                  onInput={(e) =>
                    props.setGroups(
                      groups.map((x, k) =>
                        k === i ? { ...x, count: clampCount(Number(e.currentTarget.value)) } : x,
                      ),
                    )
                  }
                  aria-label={`第 ${i + 1} 组张数`}
                />
              </label>
            </div>
          ))}
          <p class="muted small">
            这次会送出 {tasks.length} 张（每组 1–{MAX_PER_GROUP} 张，一次最多 {MAX_BATCH_TASKS}
            张）。成图进「本次画廊」，刷新即清空。
          </p>
          <div class="nai-actions">
            {phase === "idle" && (
              <button
                type="button"
                class="nai-btn nai-btn--primary"
                onClick={() => void start()}
                disabled={!tasks.length}
              >
                批量开始
              </button>
            )}
            {phase === "running" && (
              <>
                <button type="button" class="nai-btn nai-btn--primary" onClick={pause}>
                  暂停
                </button>
                <button type="button" class="nai-btn" onClick={cancelCurrent}>
                  取消当前
                </button>
              </>
            )}
            {phase === "paused" && (
              <>
                <button type="button" class="nai-btn nai-btn--primary" onClick={resume}>
                  继续
                </button>
                <button type="button" class="nai-btn" onClick={cancelCurrent}>
                  取消当前
                </button>
                <button type="button" class="nai-btn" onClick={stop}>
                  清空重来
                </button>
              </>
            )}
          </div>
        </div>
      </aside>
      <section class="nai-main" aria-label="批量进度">
        <header class="wb-queue__head">
          <p>
            共 {jobs.length} 张 · 已完成 {done}
          </p>
          {phase === "paused" && jobs.some((j) => j.status === "queued") && (
            <p class="muted small">队列已暂停：点「继续」接着跑，点「清空重来」作废没跑的。</p>
          )}
        </header>
        {jobs.length === 0 ? (
          <div class="wb-empty">
            <p>左边写好各组的提示词，点「批量开始」。</p>
            <p class="muted small">
              每组按共用词（或组内词）组合后一张一张送出；成图也会进右边的「本次画廊」。
            </p>
          </div>
        ) : (
          <ol class="wb-queue">
            {jobs.map((j, i) => (
              <li key={i} class={cx("wb-job", `wb-job--${j.status}`)}>
                <span class="wb-job__label">{list[i]?.label ?? `任务 ${i + 1}`}</span>
                <span class="wb-job__status">{STATUS_LABELS[j.status]}</span>
                {j.status === "done" && j.seconds !== undefined && (
                  <span class="muted small">{j.seconds.toFixed(1)} 秒</span>
                )}
                {j.detail && <span class="wb-job__detail small">{j.detail}</span>}
                {j.status === "done" && shots.current[i] && (
                  <button
                    type="button"
                    class="nai-btn"
                    onClick={() => props.openShot(shots.current[i] as Shot)}
                  >
                    查看
                  </button>
                )}
              </li>
            ))}
          </ol>
        )}
      </section>
    </section>
  );
}

/** 非中止失败落到哪种状态：unknown（结果待核对）单独一档，提醒人去「我的请求」核对。 */
function failureOf(e: unknown): { status: "failed" | "unknown"; message: string } {
  if (e instanceof WorkbenchError) {
    const f = classifyFailure(e.status, e);
    if (f) return { status: f.kind === "unknown" ? "unknown" : "failed", message: f.message };
    return { status: "failed", message: `限速持续，等了 3 次还没排上，这组先放弃。${e.message}` };
  }
  return { status: "failed", message: e instanceof Error ? e.message : "出图没有成功，请稍后再试。" };
}
