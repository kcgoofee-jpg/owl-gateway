/**
 * 上游账号现状：把账号保护状态（/admin/api/guard）和电量记录（/admin/api/quota-records）
 * 按账号合成一张只读表。10-31 从「运行保护」页搬到「上游」页（站长：账号状态要和上游账号放一起）。
 */
import { useState } from "preact/hooks";
import { fullTime, withUnit } from "../../../../lib/format";
import { Badge, Card } from "../../../../ui";
import { useAdmin } from "../../Admin";
import { CardPending, TableWrap } from "../_kit";
import { agoText, dash, type Guard, type Records } from "../limits/core";

export function UpstreamStatus() {
  const [expanded, setExpanded] = useState<Set<number>>(() => new Set());
  const guard = useAdmin<Guard>("/admin/api/guard");
  const records = useAdmin<Records>("/admin/api/quota-records");
  if (!guard.data || !records.data)
    return (
      <CardPending
        title="上游账号现状"
        loading={guard.loading || records.loading}
        error={guard.error ?? records.error}
      />
    );
  const rec = new Map(records.data.accounts.map((a) => [a.position, a]));
  const rows = (guard.data.accounts ?? [])
    .map((a) => ({ a, r: rec.get(a.position) }))
    // 能用的排前面，不可用的沉到最后；同组内按账号编号
    .sort((x, y) => Number(y.a.usable) - Number(x.a.usable) || x.a.position - y.a.position);
  const caps = records.data.manual.account_caps;
  const paid = records.data.manual.paid_v5;
  const now = Date.now();
  return (
    <Card title="上游账号现状">
      <TableWrap label="上游账号现状">
        <table class="table" data-testid="guard-pace">
          <thead>
            <tr>
              <th>账号</th>
              <th>状态</th>
              <th>今天 出图 / V5</th>
              <th>昨天 出图 / V5</th>
              <th>V5 电量</th>
              <th>Anlas</th>
              <th>周期</th>
              <th>最近 429</th>
              <th>排队</th>
            </tr>
          </thead>
          <tbody>
            {rows.map(({ a, r }) => (
              <tr key={a.position} class={a.usable ? "" : "up-row--muted"}>
                <td>
                  {a.usable ? (
                    `#${a.position}`
                  ) : (
                    <button
                      type="button"
                      class="link-button"
                      aria-expanded={expanded.has(a.position)}
                      onClick={() =>
                        setExpanded((prev) => {
                          const next = new Set(prev);
                          if (next.has(a.position)) next.delete(a.position);
                          else next.add(a.position);
                          return next;
                        })
                      }
                    >
                      #{a.position} {expanded.has(a.position) ? "收起" : "展开"}
                    </button>
                  )}
                </td>
                <td>
                  {!a.usable ? (
                    <Badge tone="warn">不可用</Badge>
                  ) : a.full ? (
                    <Badge tone="warn">今日已满</Badge>
                  ) : (
                    <Badge tone="ok">可用</Badge>
                  )}
                  {(a.locked_today ?? 0) > 0 && <span class="muted small"> 锁 {a.locked_today} 次</span>}
                </td>
                <td>
                  {a.usable || expanded.has(a.position)
                    ? `${withUnit(a.today, "张")} / ${r ? r.today.v5 : "—"}`
                    : "—"}
                </td>
                <td>
                  {a.usable || expanded.has(a.position)
                    ? r
                      ? `${r.yesterday.images} / ${r.yesterday.v5}`
                      : "—"
                    : "—"}
                </td>
                <td>
                  {dash(r?.v5_percent, "%")}
                  {r?.quiet_rate && (
                    <span
                      class="muted small"
                      title="相邻两次主动读之间该账号没出过 V5 图时，余量的上涨就是纯恢复"
                    >
                      （实测 {r.quiet_rate.rate}）
                    </span>
                  )}
                  <br />
                  <span class="muted small">{dash(r?.v5_recharge_per_day, "%/天")}</span>
                </td>
                <td>{dash(r?.anlas)}</td>
                <td>{dash(a.cycle, " 秒")}</td>
                <td>
                  {agoText(a.last_429, now)}
                  {a.cooling ? `（冷却还剩 ${a.cooling} 秒）` : ""}
                </td>
                <td class="queue-status">
                  {a.waiting ?? 0} 在等
                  {typeof a.queue_cap === "number" ? `，最多 ${a.queue_cap}` : ""}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </TableWrap>
      <div class="form-section" data-testid="quota-records">
        {caps.length === 0 && paid.length === 0 ? (
          <p class="muted small">没有手动临时上限或站付 V5。</p>
        ) : (
          <div class="xa-chips">
            <span class="xa-chips__lead">手动临时</span>
            {caps.map((c) => (
              <span key={`cap-${c.position}`} class="xa-chip">
                账号 {c.position ?? "?"} 每天 {c.cap} 张 · 到 {fullTime(c.until)}
              </span>
            ))}
            {paid.map((p) => (
              <span key={`paid-${p.position}`} class="xa-chip">
                账号 {p.position ?? "?"} 站付 V5 已用 {Math.round(p.spent)} / {Math.round(p.max_anlas)} Anlas
                · 到 {fullTime(p.until)}
              </span>
            ))}
          </div>
        )}
      </div>
    </Card>
  );
}
