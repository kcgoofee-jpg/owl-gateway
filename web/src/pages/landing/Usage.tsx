import { SiteGauge } from "./SiteGauge";

type Tone = "warn" | "danger" | undefined;

/** 快用满时变色：≥90% 琥珀，用满红。（成员页圆环还在用） */
export function toneFor(used: number, cap: number): Tone {
  if (cap > 0 && used >= cap) return "danger";
  if (cap > 0 && used >= cap * 0.9) return "warn";
  return undefined;
}

/** 首页图例（站长 10-27）：只说今天重置以来 V4.5 / V5 各生成了多少张，不再折算剩余额度。 */
export function StatRows(props: { generated: { total: number; v45: number; v5: number } }) {
  const rows = [
    { kind: "v45" as const, value: props.generated.v45 },
    { kind: "v5" as const, value: props.generated.v5 },
  ];
  return <SiteGauge rows={rows} total={props.generated.total} />;
}
