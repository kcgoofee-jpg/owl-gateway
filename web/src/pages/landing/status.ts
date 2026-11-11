/**
 * 首页「现在能不能出图」的推导。
 *
 * 短标题按 guard.blocked.code 映射。灰字用接口给的 when（重置说法或剩余秒数），原文没有整点时不编一个。
 */
import type { Blocked, Live } from "./types";

export type Tone = "ok" | "busy" | "full" | "mine" | "warn" | "bad";

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
/** 全站被站点规则拦下。标题按原因码，灰字用 when。 */
function blockedView(blocked: Blocked): StatusView {
  const code = blocked.code ?? "";
  const text = (blocked.text ?? "").trim();
  const when = (blocked.when ?? "").trim();
  switch (code) {
    case "daily_cap":
      return view("warn", "今日出图已满", when ? `${when}恢复` : "");
    case "breaker":
      return view("bad", blocked.manual ? "暂停出图" : "上游连续出错", when);
    default:
      return view("warn", text || "暂时无法出图");
  }
}

/** live 还没到时返回 null（页面显示骨架屏）。判断顺序：上游冷却 → 全站拦截 → 排队满 → 排队 → 空闲；自己的图优先显示。 */
export function deriveStatus(live: Live | null): StatusView | null {
  if (!live) return null;
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
    out = blockedView(blocked);
  } else if (
    // 网关在 live.queue.full 里直接给出「普通成员现在会不会以排队满拒绝」（与 guard.admit_image 同一判断；站长号可越过）
    live.queue?.full === true
  ) {
    // 站长 10-31：排队满 = 红灯；有排队但还有空位 = 黄灯；空位 = 绿灯
    out = view("full", "排队已满", `最多 ${queueCap} 张，稍后再试`);
  } else if (waiting > 0) {
    // 站长 10-27：排队状态只说一个数——当前排队张数
    out = view("busy", `排队 ${waiting} 张`);
  } else if (running > 0) {
    // 有人在出图但没人排队 = 还有空位，仍然算「能马上开始」
    out = view("ok", "正在出图");
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

/** 今天重置以来各生成了多少张 + 弧线的比例尺（各账号今日上限合计）。 */
export function generatedToday(live: Live | null): { total: number; v45: number; v5: number; cap: number } {
  const q = live?.quota ?? {};
  const total = Math.max(0, num(q.images_today));
  const v5 = Math.max(0, Math.min(total, num(q.v5_today)));
  return { total, v45: total - v5, v5, cap: Math.max(0, num(q.images_cap)) };
}
