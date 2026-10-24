import { cx } from "../../ui";
import type { MeterRow } from "./status";

export type Tone = "warn" | "danger" | undefined;

/** 快用满时变色：≥90% 琥珀，用满红。 */
export function toneFor(used: number, cap: number): Tone {
  if (cap > 0 && used >= cap) return "danger";
  if (cap > 0 && used >= cap * 0.9) return "warn";
  return undefined;
}

const fmt = (n: number) => n.toLocaleString("en-US");
const NAME = { v45: "V4.5", v5: "V5" } as const;
/** 行内小进度条的宽度（viewBox 与 CSS 各用一次，fill 的 width 属性按同一比例画）。 */
const BAR = 48;
/** 全站共享弧线：比旧版略弯（viewBox 高 40）。 */
const ARC = "M 3 31 Q 500 -9 997 31";

/**
 * 状态卡的两行仪表，统一口径「今日已出」：V4.5 行是全站今天出的总数（含 V5），V5 行是其中的 V5。
 * 上方一条全站共享弧线管整体压力：琥珀 = 全站已出/日上限，蓝 = V5 已出/同一上限（蓝段画在前、叠在琥珀前段）。
 * 每行开头是色点 + 行内小进度条，两行同一个分母（全站日上限），条的长度可比；上限为 0 时不画条、只留色点。
 * 剩余量、上限、重置时间都放进悬停提示；屏幕上只有已出数和两种例外：用满、V5 暂停。
 */
export function StatRows({ rows, reset }: { rows: MeterRow[]; reset: string }) {
  const cap = rows[0]?.cap ?? 0;
  const total = Math.max(0, rows[0]?.used ?? 0);
  const v5 = Math.max(0, Math.min(total, rows[1]?.used ?? 0));
  const usedPct = cap > 0 ? Math.min(100, (total / cap) * 100) : 0;
  const v5Pct = cap > 0 ? Math.min(usedPct, (v5 / cap) * 100) : 0;
  const arcTitle =
    cap > 0
      ? `全站今日已出 ${fmt(total)} / ${fmt(cap)} 张（含 V5 ${fmt(v5)} 张）`
      : `全站今日已出 ${fmt(total)} 张（含 V5 ${fmt(v5)} 张）`;
  return (
    <div class="lp-meters">
      <svg class="lp-site-gauge" viewBox="0 0 1000 40" preserveAspectRatio="none" aria-hidden="true">
        <title>{arcTitle}</title>
        <path class="lp-site-gauge__track" d={ARC} />
        {usedPct > 0 && (
          <path
            class="lp-site-gauge__fill stat--v45"
            d={ARC}
            pathLength={100}
            stroke-dasharray={`${usedPct} 100`}
          />
        )}
        {v5Pct > 0 && (
          <path
            class="lp-site-gauge__fill stat--v5"
            d={ARC}
            pathLength={100}
            stroke-dasharray={`${v5Pct} 100`}
          />
        )}
      </svg>
      {rows.map((r) => {
        const name = NAME[r.kind];
        const capped = r.cap > 0;
        const full = capped && r.used >= r.cap;
        const pct = capped ? Math.max(0, Math.min(100, (r.used / r.cap) * 100)) : 0;
        // 一位小数就够 48px 的条用了，也免得属性里出现浮点尾巴
        const barWidth = Math.round((pct / 100) * BAR * 10) / 10;
        const tone = r.paused ? undefined : toneFor(r.used, r.cap);
        const caption = r.paused ? "V5 暂停中" : full ? `今日已满，${reset}重置` : "";
        const title =
          r.kind === "v45"
            ? capped
              ? `今日已出 ${fmt(r.used)} / ${fmt(r.cap)} 张，${reset}重置`
              : `今日已出 ${fmt(r.used)} 张`
            : `全站今日 V5 共 ${fmt(r.used)} 张${r.hint ? `；${r.hint}` : ""}`;
        const content = (
          <>
            <span class="lp-meter__lead">
              <i aria-hidden="true" />
              {capped && (
                <svg
                  class="lp-meter__bar"
                  viewBox={`0 0 ${BAR} 6`}
                  preserveAspectRatio="none"
                  aria-hidden="true"
                >
                  <rect class="lp-meter__bar-track" width={BAR} height="6" rx="3" />
                  {!r.paused && barWidth > 0 && (
                    <rect class="lp-meter__bar-fill" width={barWidth} height="6" rx="3" />
                  )}
                </svg>
              )}
            </span>
            <span class="lp-meter__name">{name}</span>
            <b class="lp-meter__num">
              {fmt(r.used)}
              <small>张</small>
            </b>
            {caption && <span class="lp-meter__caption">{caption}</span>}
          </>
        );
        const className = cx(
          "lp-meter",
          `stat--${r.kind}`,
          tone && `stat--${tone}`,
          r.paused && "lp-meter--paused",
        );
        return capped ? (
          <div
            key={r.kind}
            class={className}
            title={title}
            role="progressbar"
            aria-label={name}
            aria-valuemin={0}
            aria-valuemax={r.cap}
            aria-valuenow={r.used}
            aria-valuetext={title}
          >
            {content}
          </div>
        ) : (
          <div key={r.kind} class={className} title={title}>
            {content}
          </div>
        );
      })}
    </div>
  );
}
