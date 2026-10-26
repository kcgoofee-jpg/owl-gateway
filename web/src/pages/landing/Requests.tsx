import { useEffect, useState } from "preact/hooks";
import { ApiError, getJSON } from "../../lib/api";
import { secTime } from "../../lib/format";
import { Badge, Button, Card, Skeleton, Tabs } from "../../ui";
import type { MyRequest, MyRequests } from "./types";

type Status = "" | "ok" | "rejected" | "error";

/** 模型 → 短名：V5 / V4.5 / 其他按请求类型。 */
export function modelLabel(r: Pick<MyRequest, "model" | "kind">): string {
  const m = r.model ?? "";
  if (/diffusion-5/.test(m)) return "V5";
  if (/diffusion-4-5/.test(m)) return "V4.5";
  if (/diffusion-4/.test(m)) return "V4";
  if (/diffusion-3|furry/.test(m)) return "V3";
  if (r.kind === "tags") return "标签补全";
  if (r.kind === "image") return "图片工具";
  return r.kind || "请求";
}

/** 成功的那条怎么算的：账号代付 / 扣 N Anlas / 用 V5 额度 / 免费；尺寸步数从说明里取。 */
export function okLine(r: Pick<MyRequest, "message" | "anlas" | "site_paid">): string {
  const size = /\d+x\d+\/\d+step/.exec(r.message)?.[0]?.replace("step", " 步");
  const cost = r.site_paid
    ? "本站账号代付，不扣你的 Anlas"
    : r.anlas > 0
      ? `扣 ${Math.round(r.anlas)} Anlas`
      : r.message.includes("V5额度")
        ? "用 V5 额度"
        : "免费";
  return [size, cost].filter(Boolean).join(" · ");
}

const STATUS: Record<string, { text: string; tone: "ok" | "warn" | "danger" }> = {
  ok: { text: "已生成", tone: "ok" },
  rejected: { text: "被拒", tone: "warn" },
  error: { text: "失败", tone: "danger" },
  cancelled: { text: "已取消", tone: "warn" },
};

/**
 * 交付徽标只留异常态：传输完成不显示——图到没到成员自己看得到，服务端也无法确认客户端收到
 * （那需要客户端回执，没有这个机制），原提示是冗余的（站长 10-27）。老记录没有交付状态，同样不提示。
 */
export function deliveryLabel(
  state: MyRequest["delivery_state"],
): { text: string; tone: "warn" | "danger" } | null {
  switch (state) {
    case "transport_interrupted":
      return { text: "传输中断", tone: "danger" };
    case "pending":
      return { text: "传输中", tone: "warn" };
    default:
      return null;
  }
}

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
    r.correlation_id ? `NovelAI 排障号 ${r.correlation_id}` : "",
  ]
    .filter(Boolean)
    .join(" · ");
  return (
    <li class="lp-req">
      <div class="lp-req__head">
        <time class="lp-req__time">{secTime(r.ts)}</time>
        <span class="lp-req__model">{modelLabel(r)}</span>
        <span class="spacer" />
        <Badge tone={s.tone}>{s.text}</Badge>
        {r.status === "ok" && pendingSettlement && (
          <Badge tone={pendingSettlement.tone}>{pendingSettlement.text}</Badge>
        )}
        {delivery && <Badge tone={delivery.tone}>{delivery.text}</Badge>}
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
      {r.status === "ok" && r.image_available && r.image_url && (
        <p class="lp-req__meta small">
          <a href={r.image_url} target="_blank" rel="noreferrer">
            取回已保存图片
          </a>
        </p>
      )}
      {meta && <p class="lp-req__meta muted small">{meta}</p>}
    </li>
  );
}

/** 一种状态下的列表：先拿第一页，「再看 50 条」往回翻。 */
function List({ status, onSummary }: { status: Status; onSummary: (s: MyRequests["summary"]) => void }) {
  const [items, setItems] = useState<MyRequest[] | null>(null);
  const [next, setNext] = useState<number | null>(null);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState("");

  const load = async (before?: number) => {
    setBusy(true);
    setErr("");
    try {
      const q = new URLSearchParams();
      if (status) q.set("status", status);
      if (before) q.set("before", String(before));
      const qs = q.toString();
      const d = await getJSON<MyRequests>(`/public/me/requests${qs ? `?${qs}` : ""}`);
      setItems((old) => (before && old ? [...old, ...d.items] : d.items));
      setNext(d.next);
      onSummary(d.summary);
    } catch (e) {
      setErr(e instanceof ApiError ? e.message : "读取失败，请稍后重试");
    } finally {
      setBusy(false);
    }
  };

  useEffect(() => {
    void load();
  }, [status]);

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
        <Button variant="ghost" class="lp-reqs__more" onClick={() => load(next)} disabled={busy}>
          {busy ? "读取中…" : "再看 50 条"}
        </Button>
      )}
    </>
  );
}

/**
 * 「我的请求」：自己上次重置（每日 7 点）以来每一次请求的结果和原因。客户端把报错改写了（比如显示成「需要订阅」）时，
 * 来这里能看到本站给的原话。
 */
export function MyRequestsCard() {
  const [sum, setSum] = useState<MyRequests["summary"] | null>(null);
  const n = (k: keyof MyRequests["summary"]) => (sum ? ` ${sum[k]}` : "");
  const tab = (id: string, text: string, k: keyof MyRequests["summary"], status: Status) => ({
    id,
    label: `${text}${n(k)}`,
    panel: () => <List status={status} onSummary={setSum} />,
  });
  return (
    // 不加 reveal：显影按「整块滚进来多少」算，长列表要滚很久才清楚
    <Card title="我的请求">
      <Tabs
        label="按结果筛选"
        tabs={[
          tab("all", "全部", "total", ""),
          tab("ok", "成功", "ok", "ok"),
          tab("rejected", "被拒", "rejected", "rejected"),
          tab("error", "失败", "error", "error"),
        ]}
      />
    </Card>
  );
}
