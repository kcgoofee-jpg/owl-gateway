import { SiteGauge } from "./SiteGauge";

type Tone = "warn" | "danger" | undefined;

/** 快用满时变色：≥90% 琥珀，用满红。 */
export function toneFor(used: number, cap: number): Tone {
  if (cap > 0 && used >= cap) return "danger";
  if (cap > 0 && used >= cap * 0.9) return "warn";
  return undefined;
}

/** 用满后换成第二天额度时的四字名称：「明天可用」/「V5 明天」（零点到重置点之间是「今天」） */
const nextName = (kind: "v45" | "v5", label: string) => {
  const day = label.includes("今天") ? "今天" : "明天";
  return kind === "v5" ? `V5 ${day}` : `${day}可用`;
};

/** Shared shallow arc; each segment retains its own allowance and reset state. */
export function StatRows(props: {
  total: { label: string; used: number; cap: number; paused?: boolean; hint?: string; short?: string };
  generated?: { total: number; v5: number };
  v5: { label: string; used: number; cap: number; paused?: boolean; hint?: string; short?: string };
}) {
  const rows = [
    { kind: "v45" as const, m: props.total, left: "V4.5 剩余", made: "V4.5 已出" },
    { kind: "v5" as const, m: props.v5, left: "V5 剩余", made: "V5 已出" },
  ].map((x) => {
    const { used, cap, label } = x.m;
    const paused = x.m.paused === true;
    const capped = cap > 0;
    // dailyMeters 在用满后换成第二天的额度：名称缩成四个字，完整说法放在悬停提示里
    const special = !paused && !label.includes("已用");
    const value = capped ? Math.max(0, cap - used) : used;
    return {
      ...x,
      capped,
      paused,
      value: paused ? 0 : value,
      name: paused ? "V5 剩余" : special ? nextName(x.kind, label) : capped ? x.left : x.made,
      tone: paused ? ("danger" as Tone) : toneFor(used, cap),
      pct: capped && !paused ? Math.max(0, Math.min(100, (value / cap) * 100)) : 0,
    };
  });
  return (
    <SiteGauge rows={rows} generated={props.generated ?? { total: props.total.used, v5: props.v5.used }} />
  );
}
