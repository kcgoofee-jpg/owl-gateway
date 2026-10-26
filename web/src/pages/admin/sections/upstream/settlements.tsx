/** Unknown dispatched requests remain held until an operator checks them. */
import { useState } from "preact/hooks";
import { sendJSON } from "../../../../lib/api";
import { Button, Card, Notice, toast } from "../../../../ui";
import { tick, useAdmin } from "../../Admin";
import { CardPending } from "../_kit";

type Settlement = {
  request_id: string;
  rid: string;
  key_id: number;
  status: string;
  service_day: string;
  service_month: string;
  reserved_anlas: number;
  reserved_v5: number;
  reserved_legacy: number;
  is_vibe: boolean;
};
type Data = { items: Settlement[]; csrf_token: string };

export default function SettlementCard() {
  const { data, error, loading } = useAdmin<Data>("/admin/api/request-settlements");
  const [reasons, setReasons] = useState<Record<string, string>>({});
  const [confirmed, setConfirmed] = useState<Record<string, boolean>>({});
  const [busy, setBusy] = useState("");
  if (!data) return <CardPending title="待核对的未知请求" loading={loading} error={error} />;

  const release = async (item: Settlement) => {
    const reason = (reasons[item.request_id] ?? "").trim();
    setBusy(item.request_id);
    try {
      await sendJSON(
        "POST",
        `/admin/api/request-settlements/${encodeURIComponent(item.request_id)}/release`,
        {
          confirmed_not_executed: true,
          reason,
        },
        { "X-NAI-Admin-CSRF": data.csrf_token },
      );
      toast("未知预留已释放并记录审计");
      tick.value++;
    } catch (e) {
      toast(e instanceof Error ? e.message : String(e), { error: true });
    } finally {
      setBusy("");
    }
  };

  return (
    <Card
      title="待核对的未知预留"
      sub="未知执行余量会持续占用额度，不会自动重发。已确认的部分结果仍按原账保留；只有核实剩余部分未执行后，才能释放该未知预留。"
    >
      {data.items.length === 0 ? (
        <p class="muted small">目前没有待核对请求。</p>
      ) : (
        <div class="stack-sm">
          {data.items.map((item) => (
            <article class="card card--inset" key={item.request_id}>
              <p class="small">
                <strong>RID {item.rid}</strong> · Key {item.key_id} · {item.service_day} /{" "}
                {item.service_month}
              </p>
              <p class="muted small">
                预留 {item.reserved_anlas} Anlas · V5 {item.reserved_v5} · 免费图 {item.reserved_legacy}
                {item.is_vibe ? " · Vibe" : ""}
              </p>
              <Notice>
                仅剩余执行结果未知。释放只解除尚未确认部分的预算预留，不撤销已确认成功部分的记账。请先核对上游记录；原因会永久写入后台审计。
              </Notice>
              <label class="field">
                <span>核对说明（至少 12 字）</span>
                <textarea
                  rows={2}
                  value={reasons[item.request_id] ?? ""}
                  onInput={(e) => setReasons({ ...reasons, [item.request_id]: e.currentTarget.value })}
                />
              </label>
              <label class="check-row">
                <input
                  type="checkbox"
                  checked={confirmed[item.request_id] ?? false}
                  onChange={(e) => setConfirmed({ ...confirmed, [item.request_id]: e.currentTarget.checked })}
                />
                <span>我已核实尚未确认的剩余部分未在上游执行</span>
              </label>
              <Button
                variant="danger"
                disabled={
                  !confirmed[item.request_id] || (reasons[item.request_id] ?? "").trim().length < 12 || !!busy
                }
                onClick={() => void release(item)}
              >
                {busy === item.request_id ? "正在记录…" : "确认未知余量未执行并释放预留"}
              </Button>
            </article>
          ))}
        </div>
      )}
    </Card>
  );
}
