/**
 * 首页「现在能不能出图」的推导（纯函数，移植自旧版 landing.html 的 render()）。
 *
 * 站长规矩：界面和接口说同一句话。网关拒绝出图时返回的 429 原因（crates/gate-core/src/guard.rs 的原因文案）
 * 同时出现在 /public/live 的 guard.blocked.text 里；这里的措辞、数字都从那句话和 live 的字段里取，
 * 不自己编（见 status.test.ts：把 guard.rs 的原文贴进去，核对两边的数字和关键词一致）。
 */
import { bjHour } from "../../lib/format";
import type { Live } from "./types";

export type Tone = "ok" | "busy" | "mine" | "warn" | "bad" | "paced";

export type StatusView = {
  tone: Tone;
  /** 一句话里的主干（粗体） */
  headline: string;
  /** 补充原因（灰字），可为空 */
  detail: string;
  /** headline · detail：测试与读屏共用的完整一句话 */
  sentence: string;
  /** 读屏播报用：不含每秒变化的倒计时，避免状态没变也一直念 */
  announce: string;
};

const view = (tone: Tone, headline: string, detail = "", announce?: string): StatusView => {
  const sentence = detail ? `${headline} · ${detail}` : headline;
  return { tone, headline, detail, sentence, announce: announce ?? sentence };
};

const num = (v: unknown): number => {
  const n = Number(v);
  return Number.isFinite(n) ? n : 0;
};
const firstInt = (text: string, re: RegExp): number | null => {
  const m = re.exec(text);
  return m?.[1] ? Number(m[1]) : null;
};

/**
 * 下一次每日重置：「今天 / 明天」和整点。原文里有就照原文（网关按当时的北京时间算好了），
 * 没有原文（例如只有 V5 满了）时按北京时间自己算：重置点之前是今天，之后是明天。
 */
function nextReset(text: string, nowMs: number): { word: string; hour: number } {
  const m = /(今天|明天)\s*(\d+)\s*点/.exec(text);
  if (m?.[1]) return { word: m[1], hour: Number(m[2]) };
  const hour = 7;
  return { word: bjHour(nowMs) < hour ? "今天" : "明天", hour };
}

/** 全站被站点规则拦下（guard.blocked）时的说法；text 就是成员收到的 429 原因。 */
function blockedView(code: string, text: string, nowMs: number): StatusView {
  const wait = firstInt(text, /约 (\d+) 分钟/);
  switch (code) {
    case "paced":
      return view("paced", "本时段份额已用完", wait ? `约 ${wait} 分钟后有空位` : "稍后有空位");
    case "daily_cap": {
      const r = nextReset(text, nowMs);
      return view("warn", "今日出图已满", `${r.word} ${r.hour} 点恢复`);
    }
    case "breaker": {
      // 自动熔断的原文很长（含「没有扣费」「请稍后再试」）：标题只说原因，灰字只留时间；站长手写的原因放灰字
      if (!text || /连续出错/.test(text)) {
        const left = firstInt(text, /约\s*(\d+)\s*秒后/);
        const total = firstInt(text, /(\d+)\s*秒/);
        return view("bad", "上游连续出错", left ? `约 ${left} 秒后恢复` : total ? `暂停 ${total} 秒` : "");
      }
      return view("bad", "暂停出图", text);
    }
    default:
      // 不认识的原因码：原文照搬，宁可直白也不改写
      return view("warn", text || "暂时无法出图");
  }
}

export type DeriveOpts = {
  nowMs?: number;
};

/** live 还没到时返回 null（页面显示骨架屏）。判断顺序：上游冷却 → 全站拦截 → 排队满 → 排队 → 空闲；自己的图优先显示。 */
export function deriveStatus(live: Live | null, opts: DeriveOpts = {}): StatusView | null {
  if (!live) return null;
  const nowMs = opts.nowMs ?? Date.now();
  const g = live.guard ?? {};
  const r = live.rules ?? {};
  const mineView = live.me ?? null;
  const q = mineView ?? live.queue ?? {};
  const waiting = num(q.waiting);
  const running = num(q.running);
  const mine = mineView?.mine ?? [];
  const myRunning = mine.some((x) => x.state === "running");
  const myWaiting = mine.find((x): x is Extract<typeof x, { state: "waiting" }> => x.state === "waiting");
  const queueCap = num(r.queue_cap);
  const blocked = g.blocked ?? null;

  let out: StatusView;
  if (num(g.cooldown) > 0) {
    // 与 auth.rs 的冷却 429 同一口径：「NovelAI 那边正在限流，全站暂停出图约 N 秒」
    out = view("bad", "NovelAI 限流中", `暂停约 ${num(g.cooldown)} 秒`);
  } else if (blocked && (blocked.code || blocked.text)) {
    out = blockedView(blocked.code ?? "", (blocked.text ?? "").trim(), nowMs);
  } else if (
    // 网关在 live.queue.full 里直接给出「现在会不会以排队满拒绝」（与 guard.admit_image 同一判断）
    live.queue?.full === true
  ) {
    out = view("warn", "排队已满", `最多 ${queueCap} 张，稍后再试`);
  } else if (waiting > 0) {
    // 站长 10-27：排队状态只说一个数——当前排队张数
    out = view("busy", `排队 ${waiting} 张`);
  } else if (running > 0) {
    out = view("busy", "正在出图");
  } else {
    out = view("ok", "马上开始", "通道空闲");
  }

  if (myRunning) {
    out = view("mine", "你的图正在生成");
  } else if (myWaiting) {
    const headline = `您排在 ${myWaiting.position}/${waiting} 位`;
    out = view("mine", headline, "", headline);
  }
  return out;
}

/** 今天重置以来各生成了多少张：图例只显示这个，不再折算「剩余额度」。 */
export function generatedToday(live: Live | null): { total: number; v45: number; v5: number } {
  const q = live?.quota ?? {};
  const total = Math.max(0, num(q.images_today));
  const v5 = Math.max(0, Math.min(total, num(q.v5_today)));
  return { total, v45: total - v5, v5 };
}

/** 站长手动暂停 V5 时的说明（成员页圆环用） */
export const V5_PAUSED_HINT = "V5 暂停，请使用 V4.5";
