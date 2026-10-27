import { cx } from "../../ui";

const ARC = "M 3 27 Q 500 -5 997 27";
const fmt = (n: number) => n.toLocaleString("en-US");

type Row = { kind: "v45" | "v5"; value: number };

/**
 * 今天重置以来全站生成了多少张：图例给出 V4.5 / V5 两个张数，
 * 弧线按「今日已出 ÷ 各账号今日上限合计」画进度（黄前蓝后），10-27 起不再折算剩余额度。
 */
export function SiteGauge({ rows, total, cap }: { rows: Row[]; total: number; cap: number }) {
  const v45 = rows.find((r) => r.kind === "v45")?.value ?? 0;
  const v5 = rows.find((r) => r.kind === "v5")?.value ?? 0;
  // 两段各占上限的百分比；V5 排在 V4.5 后面，fill 总长 = 已出 ÷ 上限
  const v45Pct = cap > 0 ? Math.min(100, (v45 / cap) * 100) : 0;
  const v5Pct = cap > 0 ? Math.min(100 - v45Pct, (v5 / cap) * 100) : 0;
  const title = `今日已出 ${total} 张，其中 V4.5 ${v45} 张、V5 ${v5} 张${
    cap > 0 ? `；各账号今日合计可出 ${fmt(cap)} 张` : ""
  }`;
  return (
    <div class="lp-meters">
      <svg class="lp-site-gauge" viewBox="0 0 1000 32" preserveAspectRatio="none" aria-hidden="true">
        <title>{title}</title>
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
      <div class="lp-meter__legend" role="img" aria-label={title}>
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
