/**
 * Anlas 消耗核对 + 待核对的未知预留，合并成一张卡。
 * 核对查询官方余额，对比「余额减少」与本站记录的消耗；未知预留要人工确认未执行后才能释放。
 * 数据全部来自页面聚合接口（props），操作完成后 tick 让整页重取。
 */
import { useEffect, useState } from "preact/hooks";
import { sendJSON } from "../../../../lib/api";
import { fullTime } from "../../../../lib/format";
import { Button, Card, Notice, toast } from "../../../../ui";
import { tick } from "../../Admin";
import { CardPending, Stat, TableWrap } from "../_kit";
import { anlasText, RECON_REASONS, type Recon, type Settlement } from "./core";

const URL = "/admin/api/reconciliation";

/** 核对接口要求带会话绑定的 CSRF 头 */
const runReconciliation = (csrf: string): Promise<Recon> =>
  sendJSON<Recon>("POST", URL, {}, { "X-NAI-Admin-CSRF": csrf });

function ReconcileSummary({ recon, csrf }: { recon: Recon; csrf: string }) {
  const [running, setRunning] = useState(false);
  const [until, setUntil] = useState(0); // 本地倒计时：冷却结束的时间点（毫秒）
  const [now, setNow] = useState(Date.now());

  useEffect(() => {
    const t = Date.now();
    setNow(t);
    setUntil(t + recon.retry_after * 1000);
  }, [recon.retry_after]);
  useEffect(() => {
    if (until <= Date.now()) return;
    const t = setInterval(() => {
      const n = Date.now();
      setNow(n);
      if (n >= until) clearInterval(t); // 倒计时到 0 就停，不再每秒重绘
    }, 1000);
    return () => clearInterval(t);
  }, [until]);

  const wait = Math.max(0, Math.ceil((until - now) / 1000));
  const latest = recon.history[0];
  const c = latest?.comparison;
  const canRun = !!csrf && !running && !recon.running && wait === 0;

  const run = async () => {
    setRunning(true);
    try {
      await runReconciliation(csrf);
      toast("核对完成");
    } catch (e) {
      toast(e instanceof Error ? e.message : String(e), { error: true });
    } finally {
      setRunning(false);
      tick.value++;
    }
  };

  return (
    <div class="stack-sm">
      <div class="row">
        <b class="small">Anlas 消耗核对</b>
        <span class="spacer" />
        <Button variant="primary" disabled={!canRun} onClick={() => void run()}>
          {running ? "正在核对…" : wait > 0 ? `${wait} 秒后核对` : "查询并核对"}
        </Button>
      </div>
      {recon.running && <Notice>正在核对…</Notice>}
      {!latest ? (
        <p class="muted small">暂无核对记录。</p>
      ) : (
        <>
          <p class="muted small" role="status">
            上次查询 {fullTime(latest.checked_at)} · {Object.keys(latest.accounts).length} 个账号 · 官方余额{" "}
            {anlasText(latest.balance)} Anlas
          </p>
          {c?.reason ? (
            <p>{RECON_REASONS[c.reason] ?? "已记录"}。</p>
          ) : c ? (
            <>
              <p class="muted small">
                核对区间：{fullTime(c.since)} — {fullTime(latest.checked_at)}
              </p>
              <div class="stats">
                <Stat label="官方余额减少" value={`${anlasText(c.balance_decrease)}`} hint="Anlas" />
                <Stat
                  label="本站记录消耗"
                  value={`${anlasText(c.recorded)}`}
                  hint={c.site_paid ? `Anlas · 含站付 V5 ${anlasText(c.site_paid)}` : "Anlas"}
                />
                <Stat label="相差" value={`${anlasText(c.difference)}`} hint="Anlas" />
              </div>
              {(c.unconfirmed_requests ?? 0) > 0 && (
                <Notice tone="warn">
                  待核对 {anlasText(c.unconfirmed_requests)} 笔 · 预估 {anlasText(c.unconfirmed_anlas)} Anlas
                </Notice>
              )}
            </>
          ) : null}
        </>
      )}
      {recon.history.length > 0 && (
        <details class="more">
          <summary class="small muted">历史记录（最近 {recon.history.length} 次）</summary>
          <TableWrap label="Anlas 核对历史">
            <table class="table">
              <caption class="sr-only">Anlas 核对历史</caption>
              <thead>
                <tr>
                  <th scope="col">查询时间</th>
                  <th scope="col" class="num">
                    官方余额
                  </th>
                  <th scope="col" class="num">
                    余额减少
                  </th>
                  <th scope="col" class="num">
                    本站记录
                  </th>
                  <th scope="col" class="num">
                    相差
                  </th>
                  <th scope="col">说明</th>
                </tr>
              </thead>
              <tbody>
                {recon.history.map((h) => (
                  <tr key={h.id}>
                    <td class="nowrap">{fullTime(h.checked_at)}</td>
                    <td class="num">{anlasText(h.balance)}</td>
                    <td class="num">{anlasText(h.comparison.balance_decrease)}</td>
                    <td class="num">{anlasText(h.comparison.recorded)}</td>
                    <td class="num">{anlasText(h.comparison.difference)}</td>
                    <td class="muted">
                      {h.comparison.reason ? (RECON_REASONS[h.comparison.reason] ?? "已记录") : "核对完成"}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </TableWrap>
        </details>
      )}
    </div>
  );
}

function Settlements({ items, csrf }: { items: Settlement[]; csrf: string }) {
  const [reasons, setReasons] = useState<Record<string, string>>({});
  const [confirmed, setConfirmed] = useState<Record<string, boolean>>({});
  const [busy, setBusy] = useState("");

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
        { "X-NAI-Admin-CSRF": csrf },
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
    <div class="stack-sm">
      <details class="up-settlements">
        <summary>待核对预留（{items.length}）</summary>
        {items.length === 0 ? (
          <p class="muted small">目前没有待核对请求。</p>
        ) : (
          items.map((item) => (
            <article class="card card--inset" key={item.request_id}>
              <p class="small">
                <strong>RID {item.rid}</strong> · Key {item.key_id} · {item.service_day} /{" "}
                {item.service_month}
              </p>
              {item.correlation_id && (
                <p class="muted small" title="本站生成并随请求发送给上游，不是上游返回的错误编号。">
                  上游关联号：{item.correlation_id}
                </p>
              )}
              <p class="muted small">
                预留 {item.reserved_anlas} Anlas · V5 {item.reserved_v5} · 免费图 {item.reserved_legacy}
                {item.is_vibe ? " · Vibe" : ""}
              </p>
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
                <span>已核实剩余部分未执行</span>
              </label>
              <Button
                variant="danger"
                disabled={
                  !confirmed[item.request_id] || (reasons[item.request_id] ?? "").trim().length < 12 || !!busy
                }
                onClick={() => void release(item)}
              >
                {busy === item.request_id ? "正在记录…" : "释放预留"}
              </Button>
            </article>
          ))
        )}
      </details>
    </div>
  );
}

export default function ReconcileCard(props: {
  recon?: Recon;
  settlements: Settlement[];
  csrf: string;
  loading: boolean;
  error?: string | null;
}) {
  if (!props.recon) {
    return <CardPending title="Anlas 核对" loading={props.loading} error={props.error} />;
  }
  return (
    <Card title="Anlas 核对">
      <div class="stack">
        <ReconcileSummary recon={props.recon} csrf={props.csrf} />
        <Settlements items={props.settlements} csrf={props.csrf} />
      </div>
    </Card>
  );
}
