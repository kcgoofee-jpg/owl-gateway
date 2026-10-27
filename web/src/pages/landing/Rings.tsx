/**
 * 已登录、有 Key 的首屏：今天的额度画成居中的圆环（行由 quotaRows 决定：V4.5、V5，Anlas 只在设了上限时出现）。
 * - 圆环里只放今天剩余的数字；弧长 = 剩余比例。停用 / 用完显示「暂停」（10-27 起没有「不限」档）。
 * - 圆环下面只有类型名（17px、600），和圆环水平居中对齐；每天总量不再显示（数字只留环里那一个）。
 * - 快用完、用完时变色沿用 toneFor；V5 能用的是个人剩余和全站剩余的较小值，全站用完时显示「暂停」。
 * - 首次出现：开屏落地后（没有开屏就立刻）弧从空画到当前、数字从 0 数上来；之后数字变了（比如用了重置券）
 *   从旧值动到新值，圆环重新充满。
 */
import { useEffect, useRef, useState } from "preact/hooks";
import { animate, EASE_OUT, reduceMotion, stagger } from "../../lib/motion";
import { type QuotaRow, quotaRows } from "./helpers";
import { splashActive } from "./Splash";
import type { MeMember } from "./types";
import { toneFor } from "./Usage";

const fmt = (n: number) => n.toLocaleString("en-US");
const TYPE: Record<QuotaRow["id"], string> = { legacy: "v45", v5: "v5", anlas: "anlas" };

/** 剩余量与弧的缺口（pathLength = 100）。 */
function shape(r: QuotaRow) {
  if (r.paused) return { left: 0, gap: 100 };
  const left = r.left ?? Math.max(0, r.cap - r.used);
  return { left, gap: r.cap > 0 ? 100 - (left / r.cap) * 100 : 0 };
}

function Ring({ row, intro }: { row: QuotaRow; intro: boolean }) {
  const paused = !!row.paused;
  const target = shape(row);
  // intro：先停在「空弧、0」，等开屏落地后再动
  const [shown, setShown] = useState(() => (intro && !paused ? { left: 0, gap: 100 } : target));
  const prev = useRef(shown);
  const playing = useRef<{ stop: () => void } | null>(null);
  const waiting = intro && splashActive.value;

  useEffect(() => {
    if (paused || waiting) return;
    const from = prev.current;
    if (from.left === target.left && from.gap === target.gap) return;
    if (reduceMotion()) {
      prev.current = target;
      setShown(target);
      return;
    }
    playing.current?.stop();
    const ctl = animate(0, 1, {
      duration: 0.9,
      ease: EASE_OUT,
      onUpdate: (t) => {
        const v = {
          left: from.left + (target.left - from.left) * t,
          gap: from.gap + (target.gap - from.gap) * t,
        };
        prev.current = v;
        setShown(v);
      },
    });
    playing.current = ctl;
    return () => ctl.stop();
  }, [target.left, target.gap, paused, waiting]);

  // 颜色按「还剩多少」算：被全站剩余量卡住时也会提前变色
  const tone = paused ? "paused" : toneFor(row.cap - target.left, row.cap);
  const type = TYPE[row.id];
  const title = paused
    ? `${row.label}：${row.hint ?? "全站今天的额度用完了"}`
    : `${row.label}：今天还剩 ${fmt(target.left)}，每天 ${fmt(row.cap)}`;
  return (
    <li class={`ring-item t-${type}`}>
      <span class="sr-only">{title}</span>
      <div class="ring" data-tone={tone} title={title} aria-hidden="true">
        <svg viewBox="0 0 104 104" aria-hidden="true">
          <circle class="ring__track" cx="52" cy="52" r="46" />
          <circle class="ring__arc" cx="52" cy="52" r="46" pathLength={100} stroke-dashoffset={shown.gap} />
        </svg>
        {paused ? (
          <span class="ring__num ring__num--paused">暂停</span>
        ) : (
          <span class="ring__num">{fmt(Math.round(shown.left))}</span>
        )}
      </div>
      <p class="ring-label" aria-hidden="true">
        <b>{row.label}</b>
      </p>
    </li>
  );
}

export function QuotaRings({ me }: { me: MeMember }) {
  const rows = quotaRows(me);
  // 第一次渲染时决定要不要「从空画起」；之后的变化都是从旧值动到新值
  const [intro] = useState(() => !reduceMotion());
  const ref = useRef<HTMLUListElement>(null);
  const waiting = intro && splashActive.value;
  const labelsDone = useRef(false);
  useEffect(() => {
    if (!intro || waiting || labelsDone.current) return;
    labelsDone.current = true;
    const labels = ref.current?.querySelectorAll(".ring-label");
    if (labels?.length)
      animate(
        [...labels],
        { opacity: [0, 1], y: [6, 0] },
        { duration: 0.4, delay: stagger(0.08, { startDelay: 0.3 }) },
      );
  }, [intro, waiting]);

  return (
    <div class="lp-quota">
      <ul class="rings" ref={ref}>
        {rows.map((r) => (
          <Ring key={r.id} row={r} intro={intro} />
        ))}
      </ul>
    </div>
  );
}
