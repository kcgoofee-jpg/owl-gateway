import { useState } from "preact/hooks";
import { Button, Card, Notice } from "../../../../ui";
import { adminWrite } from "../../data";
import type { Settlement } from "./core";

function PendingSettlement({ item, csrf }: { item: Settlement; csrf: string }) {
  const [reason, setReason] = useState("");
  const [confirmed, setConfirmed] = useState(false);
  const [pending, setPending] = useState(false);
  const [message, setMessage] = useState("");
  const chars = Array.from(reason.trim()).length;
  const release = async () => {
    setPending(true);
    setMessage("");
    const r = await adminWrite(
      "POST",
      `/admin/api/request-settlements/${encodeURIComponent(item.request_id)}/release`,
      { confirmed_not_executed: true, reason: reason.trim() },
      { notify: "return", headers: { "x-nai-admin-csrf": csrf } },
    );
    setPending(false);
    setMessage(
      r.ok ? "已释放预留；正在刷新记录。" : `未确认释放结果：${r.error}。请先刷新核对，勿直接重复操作。`,
    );
  };
  return (
    <article>
      <p>
        请求 {item.rid || item.correlation_id || "无短编号"} · Key #{item.key_id} · 服务日 {item.service_day}
      </p>
      <p>
        预留：V5 {item.reserved_v5}，V4.5 {item.reserved_legacy}，Anlas {item.reserved_anlas}；状态{" "}
        {item.status}
      </p>
      <p>
        <a
          href={`/admin/logs/usage?key=${item.key_id}${item.rid ? `&rid=${encodeURIComponent(item.rid)}` : ""}`}
        >
          查看请求日志 →
        </a>
      </p>
      <details>
        <summary>人工核对并释放</summary>
        <Notice tone="warn">
          结果未知不代表未执行。只有核实上游确未执行后才能释放，否则可能重复使用额度。
        </Notice>
        <label>
          <input
            type="checkbox"
            checked={confirmed}
            disabled={pending}
            onChange={(e) => setConfirmed(e.currentTarget.checked)}
          />{" "}
          我已核实此请求未执行
        </label>
        <label>
          核对说明（12 至 500 字）
          <textarea
            class="input"
            value={reason}
            maxLength={500}
            disabled={pending}
            onInput={(e) => setReason(e.currentTarget.value)}
          />
        </label>
        <Button disabled={pending || !confirmed || chars < 12 || chars > 500} onClick={release}>
          {pending ? "处理中…" : "释放预留"}
        </Button>
        {message && <p role="status">{message}</p>}
      </details>
    </article>
  );
}
export function Settlements({ items, csrf }: { items: Settlement[]; csrf: string }) {
  return (
    <Card title={`结果未知待核对（${items.length}）`}>
      <p class="small muted">请求可能已经在上游执行；预留不会自动释放。</p>
      {items.length === 0 ? (
        <p>没有待核对请求。</p>
      ) : (
        <div class="up-settlements">
          {items.map((item) => (
            <PendingSettlement key={item.request_id} item={item} csrf={csrf} />
          ))}
        </div>
      )}
    </Card>
  );
}
