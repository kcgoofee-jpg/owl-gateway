import { cx } from "../../ui";

const ARC = "M 3 27 Q 500 1 997 27";
const fmt = (n: number) => n.toLocaleString("en-US");

type Row = {
  kind: "v45" | "v5";
  name: string;
  capped: boolean;
  paused: boolean;
  value: number;
  pct: number;
  tone: "warn" | "danger" | undefined;
  m: { label: string; cap: number; hint?: string; short?: string };
};

/** One shared daily scale. V5 is a subset of all images, painted first from the same origin. */
export function SiteGauge({ rows, generated }: { rows: Row[]; generated: { total: number; v5: number } }) {
  const cap = rows[0]?.m.cap ?? 0;
  const total = Math.max(0, generated.total);
  const v5 = Math.max(0, Math.min(total, generated.v5));
  const usedPct = cap > 0 ? Math.min(100, (total / cap) * 100) : 0;
  const v5Pct = cap > 0 ? Math.min(usedPct, (v5 / cap) * 100) : 0;
  return (
    <div class="lp-meters">
      <svg class="lp-site-gauge" viewBox="0 0 1000 32" preserveAspectRatio="none" aria-hidden="true">
        <title>{`V5 在前，V4.5 在后；今日总量 ${total} 张，V5 ${v5} 张`}</title>
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
      <div class="lp-meter__legend">
        {rows.map((r) => {
          const future = !r.paused && !r.m.label.includes("已用");
          const caption = r.paused
            ? (r.m.short ?? "暂时用完")
            : future
              ? (r.m.label.match(/（(.+)）/)?.[1] ?? r.name)
              : !r.capped
                ? ""
                : r.pct <= 10
                  ? `仅剩 ${Math.round(r.pct)}%`
                  : "";
          const description = r.paused
            ? (r.m.hint ?? "V5 暂时用完")
            : r.capped
              ? `${r.name} ${fmt(r.value)} 张，每日 ${fmt(r.m.cap)} 张${caption ? `；${caption}` : ""}`
              : `${r.name} ${fmt(r.value)} 张`;
          const content = (
            <>
              <span class="lp-meter__name">
                <i aria-hidden="true" />
                {r.kind === "v45" ? "V4.5" : "V5"}
              </span>
              <b class="lp-meter__num">
                {r.paused ? caption : fmt(r.value)}
                {!r.paused && <small>张</small>}
              </b>
              {!r.paused && caption && <span class="lp-meter__caption">{caption}</span>}
            </>
          );
          const className = cx(
            "lp-meter",
            `stat--${r.kind}`,
            r.tone && `stat--${r.tone}`,
            r.paused && "lp-meter--paused",
          );
          return r.capped ? (
            <div
              key={r.kind}
              class={className}
              title={description}
              role="progressbar"
              aria-label={r.kind === "v45" ? "V4.5" : "V5"}
              aria-valuemin={0}
              aria-valuemax={r.m.cap}
              aria-valuenow={r.value}
              aria-valuetext={description}
            >
              {content}
            </div>
          ) : (
            <div key={r.kind} class={className} title={description}>
              {content}
            </div>
          );
        })}
      </div>
    </div>
  );
}
