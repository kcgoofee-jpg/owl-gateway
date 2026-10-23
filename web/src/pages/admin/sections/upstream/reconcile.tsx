/** Anlas 消耗核对：查询官方余额，对比「余额减少」与本站记录的消耗。 */
import { useEffect, useState } from "preact/hooks";
import { sendJSON } from "../../../../lib/api";
import { fullTime } from "../../../../lib/format";
import { Button, Card, Notice, toast } from "../../../../ui";
import { tick, useAdmin } from "../../Admin";
import { CardPending, Stat, TableWrap } from "../_kit";
import { anlasText, RECON_REASONS, type Recon } from "./core";

const URL = "/admin/api/reconciliation";

/** 核对接口要求带会话绑定的 CSRF 头 */
const runReconciliation = (csrf: string): Promise<Recon> =>
  sendJSON<Recon>("POST", URL, {}, { "X-NAI-Admin-CSRF": csrf });

export default function ReconcileCard() {
  const { data, error, loading } = useAdmin<Recon>(URL);
  const [running, setRunning] = useState(false);
  const [until, setUntil] = useState(0); // 本地倒计时：冷却结束的时间点（毫秒）
  const [now, setNow] = useState(Date.now());

  useEffect(() => {
    if (!data) return;
    // 两个时间点取同一刻：冷却已结束时下面的计时器不会启动，若 now 还停在挂载那一刻，
    // 按钮会一直显示「N 秒后可再次查询」（N = 挂载到数据返回的秒数）而点不了
    const t = Date.now();
    setNow(t);
    setUntil(t + data.retry_after * 1000);
  }, [data]);
  useEffect(() => {
    if (until <= Date.now()) return;
    const t = setInterval(() => {
      const n = Date.now();
      setNow(n);
      if (n >= until) clearInterval(t); // 倒计时到 0 就停，不再每秒重绘
    }, 1000);
    return () => clearInterval(t);
  }, [until]);

  if (!data) return <CardPending title="Anlas 消耗核对" loading={loading} error={error} />;

  const wait = Math.max(0, Math.ceil((until - now) / 1000));
  const latest = data.history[0];
  const c = latest?.comparison;
  const canRun = !!data.csrf_token && !running && !data.running && wait === 0;

  const run = async () => {
    setRunning(true);
    try {
      await runReconciliation(data.csrf_token);
      toast("核对完成");
    } catch (e) {
      toast(e instanceof Error ? e.message : String(e), { error: true });
    } finally {
      setRunning(false);
      tick.value++;
    }
  };

  return (
    <Card
      title="Anlas 消耗核对"
      sub="对比官方余额的减少量与本站记录的消耗。充值、续费或在别处使用账号也会影响余额；核对不改任何成员的额度。"
      actions={
        <Button variant="primary" disabled={!canRun} onClick={() => void run()}>
          {running ? "正在核对…" : wait > 0 ? `${wait} 秒后可再次查询` : "查询并核对"}
        </Button>
      }
    >
      <div class="stack-sm">
        {data.running && <Notice>其他页面正在核对，稍后这里会自动出现结果。</Notice>}
        {!latest ? (
          <p class="muted small">还没有记录。点「查询并核对」获取当前余额，之后再次查询才会有对比。</p>
        ) : (
          <>
            <p class="muted small" role="status">
              上次查询 {fullTime(latest.checked_at)} · {Object.keys(latest.accounts).length} 个账号 · 官方余额{" "}
              {anlasText(latest.balance)} Anlas
            </p>
            {c?.reason ? (
              <p>{RECON_REASONS[c.reason] ?? "已记下当前余额"}。下次查询时，会核对这期间用了多少 Anlas。</p>
            ) : c ? (
              <>
                <p class="muted small">
                  这次核对的时间段：{fullTime(c.since)} — {fullTime(latest.checked_at)}
                </p>
                <div class="stats">
                  <Stat label="官方余额减少" value={`${anlasText(c.balance_decrease)}`} hint="Anlas" />
                  <Stat
                    label="本站记录消耗"
                    value={`${anlasText(c.recorded)}`}
                    hint={c.site_paid ? `Anlas · 含站付 V5 ${anlasText(c.site_paid)}` : "Anlas"}
                  />
                  <Stat label="相差" value={`${anlasText(c.difference)}`} hint="官方减少 − 本站记录" />
                </div>
                <p>
                  {c.difference === 0
                    ? "余额变化与本站记录一致。"
                    : `官方余额减少了 ${anlasText(c.balance_decrease)} Anlas，比本站记录${(c.difference ?? 0) > 0 ? "多" : "少"} ${anlasText(Math.abs(c.difference ?? 0))} Anlas。`}
                </p>
                {(c.unconfirmed_requests ?? 0) > 0 && (
                  <Notice tone="warn">
                    有 {anlasText(c.unconfirmed_requests)} 笔失败请求可能产生费用，合计预估{" "}
                    {anlasText(c.unconfirmed_anlas)} Anlas。
                  </Notice>
                )}
              </>
            ) : null}
          </>
        )}
        {data.history.length > 0 && (
          <details class="more">
            <summary class="small muted">历史记录（最近 {data.history.length} 次）</summary>
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
                  {data.history.map((h) => (
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
    </Card>
  );
}
