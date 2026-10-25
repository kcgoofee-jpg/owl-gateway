/** V5 官方余量：每个账号剩余百分比、恢复速度（记录，只显示）。 */
import { fullTime } from "../../../../lib/format";
import { Badge, Card, Meter, Notice } from "../../../../ui";
import { useAdmin } from "../../Admin";
import { LoadError, Loading } from "../_kit";
import { withTone } from "../system/forms";
import type { Allowance, AllowanceAccount } from "./core";

function Account(props: { a: AllowanceAccount; threshold: number }) {
  const a = props.a;
  const exhausted = a.is_negative === true;
  const tone = exhausted ? "danger" : a.low ? "warn" : undefined;
  return (
    <div class="stack-sm">
      <div class="row">
        <b>账号 {a.account}</b>
        {exhausted && <Badge tone="danger">已耗尽</Badge>}
        {!exhausted && a.low && <Badge tone="warn">低于 {props.threshold}%</Badge>}
        {a.uncertain && a.percent != null && <Badge>上次记录，待下次生成确认</Badge>}
        {a.checked_at && <span class="muted small">更新于 {fullTime(a.checked_at)}</span>}
      </div>
      <p class="muted small">
        订阅：{a.active == null ? "未知" : a.active ? "有效" : "无效"} · tier {a.tier ?? "未知"}
        {a.expires_at ? ` · 到期 ${fullTime(a.expires_at)}` : " · 到期未知"}
        {a.is_grace_period == null ? " · 宽限期未知" : a.is_grace_period ? " · 宽限期" : ""}
      </p>
      {a.percent == null ? (
        <p class="muted small">还没有读数，首次 V5 生图时会确认。</p>
      ) : (
        <Meter
          label="V5 剩余"
          value={exhausted ? 0 : a.percent}
          max={100}
          text={exhausted ? "已耗尽" : `${a.percent}%`}
          {...withTone(tone)}
        />
      )}
      <p class="muted small">
        购买 V5 {a.purchased_percent == null ? "未知" : `${a.purchased_percent}%`} · 固定 Anlas{" "}
        {a.fixed_anlas ?? "未知"} · 购买 Anlas {a.purchased_anlas ?? "未知"}
      </p>
      <p class="muted small">
        官方原始读数：到下一个 1% 点{" "}
        {a.time_until_next_percent_seconds == null ? "未知" : `${a.time_until_next_percent_seconds} 秒`}
        {a.time_until_next_percent_seconds === 0 ? "（当前暂停恢复）" : ""}。当前响应外推估算：
        {a.current_response_recharge_per_day_estimate == null
          ? "未知"
          : `${a.current_response_recharge_per_day_estimate}%/天`}
        （按 86400/秒计算，不是官方速率或保证）。历史样本外推估算：
        {a.historical_sample_based_recharge_per_day_estimate == null
          ? "未知"
          : `${a.historical_sample_based_recharge_per_day_estimate}%/天`}
        。
      </p>
      {a.error && <Notice tone="warn">{a.error}：V5 生图需确认后继续。</Notice>}
    </div>
  );
}

export default function AllowanceCard() {
  const { data, error, loading } = useAdmin<Allowance>("/admin/api/allowance");
  return (
    <Card
      title="V5 官方余量"
      sub={
        data
          ? `余量低于 ${data.threshold}% 时告警（阈值在「额度与保护」里改）。读数只在有 V5 生图或定时查询时更新。`
          : undefined
      }
    >
      {loading && !data && <Loading rows={1} />}
      {!data && error && <LoadError error={error} />}
      {data && !data.accounts.length && <p class="muted small">未配置上游账号。</p>}
      <div class="stack">
        {data?.accounts.map((a) => (
          <Account key={a.account} a={a} threshold={data.threshold} />
        ))}
      </div>
    </Card>
  );
}
