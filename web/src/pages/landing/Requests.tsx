import { secTime } from "../../lib/format";
import { deliveryLabel, modelLabel, TOOL_LABELS as TOOL_KINDS } from "../../lib/request-label";
import { Badge, Button, Card, Skeleton, Tabs } from "../../ui";
import { RequestStatsPanel } from "./RequestStats";
import type { MyRequest, MyRequests } from "./types";
import { useMyRequests } from "./useMyRequests";

type Status = "" | "ok" | "rejected" | "error";

export { deliveryLabel, modelLabel } from "../../lib/request-label";

const BILLING_TEXT = {
  site_paid: "本站账号代付，不扣你的 Anlas",
  v5: "用 V5 额度",
  free: "免费",
} as const;

function fallbackSize(message: string): string {
  const raw = /(\d+)x(\d+)\/(\d+)step/.exec(message);
  return raw ? `${raw[1]} × ${raw[2]} / ${raw[3]} 步` : "";
}

function fallbackBilling(
  r: Pick<MyRequest, "message" | "anlas" | "site_paid">,
): keyof typeof BILLING_TEXT | "anlas" {
  if (r.site_paid) return "site_paid";
  if (r.anlas > 0) return "anlas";
  if (r.message.includes("V5额度")) return "v5";
  return "free";
}

function costText(r: Pick<MyRequest, "anlas" | "billing">, billing: string): string {
  if (billing === "anlas") return `扣 ${Math.round(r.anlas)} Anlas`;
  return BILLING_TEXT[billing as keyof typeof BILLING_TEXT] ?? "";
}

/** 成功的那条怎么算的：接口给了 `size` / `billing` 就直接拼；老记录再从说明里认。 */
export function okLine(
  r: Pick<MyRequest, "message" | "anlas" | "site_paid" | "kind" | "size" | "billing">,
): string {
  const size = typeof r.size === "string" ? r.size : fallbackSize(r.message);
  const billing = r.billing ?? fallbackBilling(r);
  const cost = costText(r, billing);
  const tool = TOOL_KINDS[r.kind ?? ""] ?? "";
  const lead = r.kind === "upscale" || r.kind === "augment-image" || r.kind === "vibe_encode" ? tool : "";
  return [lead, size, cost].filter(Boolean).join("  ·  ");
}

const STATUS: Record<string, { text: string; tone: "ok" | "warn" | "danger" }> = {
  ok: { text: "已生成", tone: "ok" },
  rejected: { text: "被拒", tone: "warn" },
  error: { text: "失败", tone: "danger" },
  cancelled: { text: "已取消", tone: "warn" },
};

export function settlementLabel(
  status: MyRequest["settlement_status"],
): { text: string; tone: "warn" | "danger" } | null {
  if (status !== "uncertain") return null;
  return { text: "执行结果待核对 · 有未知余量", tone: "warn" };
}

export function RequestRow({ r }: { r: MyRequest }) {
  const pendingSettlement = settlementLabel(r.settlement_status);
  const s =
    pendingSettlement && r.status !== "ok"
      ? { text: "执行结果待核对", tone: "warn" as const }
      : (STATUS[r.status] ?? { text: r.status, tone: "warn" as const });
  const delivery = r.status === "ok" ? deliveryLabel(r.delivery_state) : null;
  const meta = [
    r.wait_s != null && r.wait_s >= 1 ? `排队 ${Math.round(r.wait_s)} 秒` : "",
    r.dur_s != null ? `出图 ${Math.round(r.dur_s)} 秒` : "",
    r.rid ? `编号 ${r.rid}` : "",
  ]
    .filter(Boolean)
    .join(" · ");
  return (
    <li class="lp-req">
      <div class="lp-req__head">
        <time class="lp-req__time">{secTime(r.ts)}</time>
        <span class="lp-req__model">{modelLabel(r)}</span>
        <span class="lp-req__app" data-app={r.app === "workbench" ? "workbench" : "client"}>
          {r.app === "workbench" ? "workbench" : "客户端"}
        </span>
        <span class="spacer" />
        <Badge tone={s.tone}>{s.text}</Badge>
        {r.status === "ok" && pendingSettlement && (
          <Badge tone={pendingSettlement.tone}>{pendingSettlement.text}</Badge>
        )}
        {delivery && (
          <Badge
            tone={delivery.tone}
            title="只表示网关没有完整交付响应，不能据此判断由浏览器、插件或网络哪一端断开。"
          >
            {delivery.text}
          </Badge>
        )}
      </div>
      {r.status === "ok" ? (
        <p class="lp-req__line">{okLine(r)}</p>
      ) : (
        <p class="lp-req__line">
          {r.code != null && <span class="lp-req__code">{r.code}</span>}
          {r.message || "无说明"}
        </p>
      )}
      {pendingSettlement && (
        <p class="lp-req__meta small">
          已确认的生成结果仍按上方记录；仍待核对余量：{r.remaining_anlas ?? 0} Anlas · V5{" "}
          {r.remaining_v5 ?? 0} · 免费图 {r.remaining_legacy ?? 0}。请勿自动重发，先核对请求记录。
        </p>
      )}
      {meta && <p class="lp-req__meta muted small">{meta}</p>}
    </li>
  );
}

function RequestList(props: {
  items: MyRequest[] | null;
  next: number | null;
  busy: boolean;
  err: string;
  onMore: () => void;
}) {
  const { items, next, busy, err, onMore } = props;
  if (items === null && !err) return <Skeleton h="120px" />;
  return (
    <>
      {err && <p class="muted small">{err}</p>}
      {items && items.length === 0 && <p class="muted small">暂无</p>}
      {items && items.length > 0 && (
        <ul class="lp-reqs">
          {items.map((r) => (
            <RequestRow key={r.id} r={r} />
          ))}
        </ul>
      )}
      {next != null && (
        <Button variant="ghost" class="lp-reqs__more" onClick={onMore} disabled={busy}>
          {busy ? "读取中…" : "再看 200 条"}
        </Button>
      )}
    </>
  );
}

type TabSpec = { id: string; text: string; key: keyof MyRequests["summary"]; status: Status };

const TABS: TabSpec[] = [
  { id: "all", text: "全部", key: "total", status: "" },
  { id: "ok", text: "成功", key: "ok", status: "ok" },
  { id: "rejected", text: "被拒", key: "rejected", status: "rejected" },
  { id: "error", text: "失败", key: "error", status: "error" },
];

/**
 * 「我的请求」：自己上次重置（每日 7 点）以来每一次请求的结果和原因。客户端把报错改写了（比如显示成「需要订阅」）时，
 * 来这里能看到本站给的原话。四个结果 tab 在本地筛；统计第一次点开才读。
 */
export function MyRequestsCard() {
  const q = useMyRequests();
  const n = (k: keyof MyRequests["summary"]) => (q.sum ? ` ${q.sum[k]}` : "");
  const tabs = TABS.map((t) => ({
    id: t.id,
    label: `${t.text}${n(t.key)}`,
    panel: () => (
      <RequestList
        items={q.rows === null ? null : t.status ? q.rows.filter((r) => r.status === t.status) : q.rows}
        next={q.rows === null ? null : q.next}
        busy={q.busy}
        err={q.err}
        onMore={q.loadMore}
      />
    ),
  }));
  return (
    <Card title="我的请求">
      <Tabs
        label="按结果筛选"
        onSelect={(id) => id === "stats" && q.openStats()}
        tabs={[
          ...tabs,
          {
            id: "stats",
            label: "统计",
            panel: () => <RequestStatsPanel data={q.stats} error={q.statsErr} onRetry={q.retryStats} />,
          },
        ]}
      />
    </Card>
  );
}
