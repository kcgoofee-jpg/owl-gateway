import { cx } from "../../ui";

const ARC = "M 3 27 Q 500 -5 997 27";
const fmt = (n: number) => n.toLocaleString("en-US");

type Row = { kind: "v45" | "v5"; value: number };

/**
 * 今天重置以来全站生成了多少张：一条共享的浅弧按 V4.5 / V5 的占比分段，图例给出两个张数。
 * 10-27 起不再显示「剩余额度」：上限与暂停只在拒绝原因里出现，看板只报已经发生的事。
 */
export function SiteGauge({ rows, total }: { rows: Row[]; total: number }) {
  const sum = Math.max(1, total);
  const v45 = rows.find((r) => r.kind === "v45")?.value ?? 0;
  const v5 = rows.find((r) => r.kind === "v5")?.value ?? 0;
  const v45Pct = total > 0 ? (v45 / sum) * 100 : 0;
  const v5Pct = total > 0 ? (v5 / sum) * 100 : 0;
  return (
    <div class="lp-meters">
      <svg class="lp-site-gauge" viewBox="0 0 1000 32" preserveAspectRatio="none" aria-hidden="true">
        <title>{`今日已出 ${total} 张，其中 V4.5 ${v45} 张、V5 ${v5} 张`}</title>
        <path class="lp-site-gauge__track" d={ARC} />
        {v45Pct > 0 && (
          <path
            class="lp-site-gauge__fill stat--v45"
            d={ARC}
            pathLength={100}
            stroke-dasharray={`${v45Pct} 100`}
          />
        )}
        {v5Pct > 0 && (
          <path
            class="lp-site-gauge__fill stat--v5"
            d={ARC}
            pathLength={100}
            stroke-dasharray={`${v5Pct} 100`}
            stroke-dashoffset={`${-v45Pct}`}
          />
        )}
      </svg>
      <div class="lp-meter__legend">
        {rows.map((r) => (
          <div
            key={r.kind}
            class={cx("lp-meter", `stat--${r.kind}`)}
            title={`今天重置以来 ${r.kind === "v45" ? "V4.5" : "V5"} 已生成 ${fmt(r.value)} 张`}
          >
            <span class="lp-meter__name">
              <i aria-hidden="true" />
              {r.kind === "v45" ? "V4.5" : "V5"} 已出
            </span>
            <b class="lp-meter__num">
              {fmt(r.value)}
              <small>张</small>
            </b>
          </div>
        ))}
      </div>
    </div>
  );
}
