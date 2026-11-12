import { fullTime, num } from "../../lib/format";
import { modelLabel } from "../../lib/request-label";
import { Button, Skeleton } from "../../ui";
import type { MyRequestStats } from "./types";
import "./request-stats.css";

export function requestCategories(rows: MyRequestStats["breakdown"]) {
  const counts = new Map<string, number>();
  for (const row of rows) {
    const label = modelLabel(row);
    counts.set(label, (counts.get(label) ?? 0) + row.requests);
  }
  return [...counts].sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]));
}

function Metric({ label, value, unit }: { label: string; value: string; unit?: string | undefined }) {
  return (
    <div class="lp-metric">
      <dt>{label}</dt>
      <dd>
        {value}
        {unit && <span class="lp-metric__unit">{unit}</span>}
      </dd>
    </div>
  );
}

function Bars({ rows, total }: { rows: Array<[string, number, string?]>; total: number }) {
  return (
    <ul class="lp-stat-bars">
      {rows.map(([label, count, tone]) => (
        <li key={label}>
          <div class="lp-stat-bars__label">
            <span>{label}</span>
            <span>{num(count)} 次</span>
          </div>
          <div class="lp-stat-bars__track" aria-hidden="true">
            <span data-tone={tone} style={{ width: `${total ? (count / total) * 100 : 0}%` }} />
          </div>
        </li>
      ))}
    </ul>
  );
}

export function RequestStatsView({ data }: { data: MyRequestStats }) {
  const s = data.summary;
  const secs = (value: number | null) => (value == null ? "—" : num(Math.round(value * 10) / 10));
  return (
    <div class="lp-request-stats">
      <p class="lp-request-stats__range muted small">{fullTime(data.since)} 起 · 北京时间每日 07:00 重置</p>
      <dl class="lp-metrics">
        <Metric label="请求总数" value={num(s.total)} unit="次" />
        <Metric label="成功率" value={s.total ? `${((s.ok / s.total) * 100).toFixed(1)}%` : "—"} />
        <Metric label="成功生成图片" value={num(data.images)} unit="张" />
        <Metric label="已消耗 Anlas" value={num(data.anlas)} />
        <Metric
          label="平均排队"
          value={secs(data.avg_wait_s)}
          unit={data.avg_wait_s == null ? undefined : "秒"}
        />
        <Metric
          label="平均生成"
          value={secs(data.avg_dur_s)}
          unit={data.avg_dur_s == null ? undefined : "秒"}
        />
      </dl>
      {s.total === 0 ? (
        <p class="muted lp-request-stats__empty">本服务日暂无请求</p>
      ) : (
        <div class="lp-request-stats__charts">
          <section aria-label="请求结果分布">
            <h3>请求结果</h3>
            <Bars
              total={s.total}
              rows={[
                ["成功", s.ok, "ok"],
                ["被拒", s.rejected, "warn"],
                ["失败", s.error, "danger"],
                ["已取消", s.cancelled ?? 0, "muted"],
                ...(s.total > s.ok + s.rejected + s.error + (s.cancelled ?? 0)
                  ? [["其他", s.total - s.ok - s.rejected - s.error - (s.cancelled ?? 0)] as [string, number]]
                  : []),
              ]}
            />
          </section>
          <section aria-label="模型与工具请求分布">
            <h3>模型与工具</h3>
            <Bars total={s.total} rows={requestCategories(data.breakdown)} />
          </section>
        </div>
      )}
      <p class="muted small lp-request-stats__note">
        成功率 = 成功请求数 ÷ 请求总数。图片数按成功记录计；耗时只统计有有效计时的成功生图请求。Anlas
        不含站点代付及待核对的预留金额。
      </p>
      {data.uncertain_requests > 0 && (
        <p class="lp-request-stats__uncertain" role="status">
          {data.uncertain_requests} 次请求仍有执行结果待核对，请结合请求记录查看。
        </p>
      )}
    </div>
  );
}

/** 统计面板：数据由卡片持有（第一次点开才读、切走再回来不重读），这里只负责显示和重试。 */
export function RequestStatsPanel(props: {
  data: MyRequestStats | null;
  error: string;
  onRetry: () => void;
}) {
  const { data, error, onRetry } = props;
  return (
    <>
      {error && (
        <div class="lp-request-stats__error" role="alert">
          <p>
            {error}
            {data && "；当前显示上次读取的数据。"}
          </p>
          <Button variant="ghost" onClick={onRetry}>
            重试
          </Button>
        </div>
      )}
      {data ? <RequestStatsView data={data} /> : !error && <Skeleton h="320px" />}
    </>
  );
}
