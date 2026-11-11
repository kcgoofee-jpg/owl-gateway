import { useEffect, useRef, useState } from "preact/hooks";
import { ApiError, getJSON } from "../../lib/api";
import { secTime } from "../../lib/format";
import { deliveryLabel, modelLabel, TOOL_LABELS as TOOL_KINDS } from "../../lib/request-label";
import { Badge, Button, Card, Skeleton, Tabs } from "../../ui";
import { RequestStatsPanel } from "./RequestStats";
import type { MyRequest, MyRequestStats, MyRequests } from "./types";

type Status = "" | "ok" | "rejected" | "error";

export { deliveryLabel, modelLabel } from "../../lib/request-label";

/** 成功的那条怎么算的：账号代付 / 扣 N Anlas / 用 V5 额度 / free；尺寸步数从说明里取。 */
export function okLine(r: Pick<MyRequest, "message" | "anlas" | "site_paid" | "kind">): string {
  // 832x1216/23step → 832 × 1216 / 23 步（数字与符号之间留空格，中英文混排都好看）
  const raw = /(\d+)x(\d+)\/(\d+)step/.exec(r.message);
  const size = raw ? `${raw[1]} × ${raw[2]} / ${raw[3]} 步` : "";
  const cost = r.site_paid
    ? "本站账号代付，不扣你的 Anlas"
    : r.anlas > 0
      ? `扣 ${Math.round(r.anlas)} Anlas`
      : r.message.includes("V5额度")
        ? "用 V5 额度"
        : "free";
  // 工具类请求没有尺寸步数，把工具名也说清楚（「扣 2 Anlas」看不出是 Vibe 编码）
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
  // 次要信息合并成一行灰字，标题行只留时间和状态（10-27 排版精简）
  const meta = [
    r.wait_s != null && r.wait_s >= 1 ? `排队 ${Math.round(r.wait_s)} 秒` : "",
    r.dur_s != null ? `出图 ${Math.round(r.dur_s)} 秒` : "",
    r.rid ? `编号 ${r.rid}` : "",
    r.correlation_id ? `上游关联号 ${r.correlation_id}` : "",
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

/** 一屏列表：数据由卡片统一持有，切 tab 只换筛选条件，不再发请求。 */
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

/** 一张卡里的一个 tab：只记 id、标题、按哪个状态筛。 */
type TabSpec = { id: string; text: string; key: keyof MyRequests["summary"]; status: Status };

const TABS: TabSpec[] = [
  { id: "all", text: "全部", key: "total", status: "" },
  { id: "ok", text: "成功", key: "ok", status: "ok" },
  { id: "rejected", text: "被拒", key: "rejected", status: "rejected" },
  { id: "error", text: "失败", key: "error", status: "error" },
];

/**
 * 「我的请求」：自己上次重置（每日 7 点）以来每一次请求的结果和原因。客户端把报错改写了（比如显示成「需要订阅」）时，
 * 来这里能看到本站给的原话。
 *
 * 一次拉整个服务日（最多 200 条，够覆盖一天），四个结果 tab 在本地筛：切 tab 是纯前端的事，
 * 不发请求、不闪骨架。统计另算一个请求，第一次点开才发，之后用缓存。
 */
export function MyRequestsCard() {
  const [rows, setRows] = useState<MyRequest[] | null>(null);
  const [next, setNext] = useState<number | null>(null);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState("");
  const [sum, setSum] = useState<MyRequests["summary"] | null>(null);
  const [stats, setStats] = useState<MyRequestStats | null>(null);
  const [statsErr, setStatsErr] = useState("");
  const [statsOpened, setStatsOpened] = useState(false);
  /** 「重试」只递增这个：统计那个 effect 按它重跑一次 */
  const [statsRetry, setStatsRetry] = useState(0);
  /** 点过「再看 200 条」就不再自动刷新，避免打乱正在看的历史位置 */
  const paged = useRef(false);

  const load = async (before?: number) => {
    setBusy(true);
    setErr("");
    if (before) paged.current = true;
    try {
      const d = await getJSON<MyRequests>(`/public/me/requests${before ? `?before=${before}` : ""}`);
      // 翻页时追加；否则整替换（自动刷新走这条）
      setRows((old) => (before && old ? [...old, ...d.items] : d.items));
      setNext(d.next);
      setSum(d.summary);
    } catch (e) {
      setErr(e instanceof ApiError ? e.message : "读取失败，请稍后重试");
    } finally {
      setBusy(false);
    }
  };

  useEffect(() => {
    void load();
  }, []);

  // 自动刷新（站长 10-27）：页面可见时每 60 秒重读第一页；切回标签页 / 窗口聚焦时立刻读一次。
  useEffect(() => {
    const refresh = () => {
      if (paged.current || document.visibilityState !== "visible") return;
      void load();
    };
    const timer = setInterval(refresh, 60_000);
    const onVis = () => document.visibilityState === "visible" && refresh();
    document.addEventListener("visibilitychange", onVis);
    window.addEventListener("focus", refresh);
    return () => {
      clearInterval(timer);
      document.removeEventListener("visibilitychange", onVis);
      window.removeEventListener("focus", refresh);
    };
  }, []);

  // 统计：第一次点开才读，之后留在 state 里（切走再回来不发请求）；60 秒重读一次只在可见时。
  const openStats = () => setStatsOpened(true);
  useEffect(() => {
    if (!statsOpened) return;
    let alive = true;
    const loadStats = async () => {
      if (document.visibilityState === "hidden") return;
      try {
        const result = await getJSON<MyRequestStats>("/public/me/request-stats");
        if (!alive) return;
        setStats(result);
        setStatsErr("");
        setSum(result.summary);
      } catch (e) {
        if (alive) setStatsErr(e instanceof ApiError ? e.message : "统计读取失败，请稍后重试");
      }
    };
    void loadStats();
    const timer = setInterval(() => void loadStats(), 60_000);
    return () => {
      alive = false;
      clearInterval(timer);
    };
  }, [statsOpened, statsRetry]);

  const n = (k: keyof MyRequests["summary"]) => (sum ? ` ${sum[k]}` : "");
  const tabs = TABS.map((t) => ({
    id: t.id,
    label: `${t.text}${n(t.key)}`,
    panel: () => (
      <RequestList
        items={rows === null ? null : t.status ? rows.filter((r) => r.status === t.status) : rows}
        next={rows === null ? null : next}
        busy={busy}
        err={err}
        onMore={() => void load(next ?? undefined)}
      />
    ),
  }));
  return (
    // 不加 reveal：显影按「整块滚进来多少」算，长列表要滚很久才清楚
    <Card title="我的请求">
      <Tabs
        label="按结果筛选"
        onSelect={(id) => id === "stats" && openStats()}
        tabs={[
          ...tabs,
          {
            id: "stats",
            label: "统计",
            panel: () => (
              <RequestStatsPanel
                data={stats}
                error={statsErr}
                onRetry={() => {
                  setStats(null);
                  setStatsErr("");
                  setStatsRetry((n) => n + 1);
                }}
              />
            ),
          },
        ]}
      />
    </Card>
  );
}
