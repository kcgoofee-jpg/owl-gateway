/** 账号对比：每个上游账号最近 1 小时 / 24 小时的请求、上游错误，以及推算的 Anlas 余额与续费倒计时。 */
import { fullTime } from "../../../../lib/format";
import { Badge, Card, Notice } from "../../../../ui";
import { useAdmin } from "../../Admin";
import { LoadError, Loading } from "../_kit";
import { Kv } from "../system/forms";
import { anlasText } from "./core";
import {
  type AccountStats,
  ageText,
  daysLeftText,
  requestsText,
  type UpstreamStats,
  upstreamErrorsText,
} from "./stats";

function Account(props: { a: AccountStats }) {
  const { a } = props;
  const b = a.balance;
  return (
    <div class="token">
      <div class="token__head">
        <span class="token__name">账号 {a.position}</span>
        {!a.usable && <Badge tone="warn">当前不可用</Badge>}
        {a.usable && a.full && <Badge tone="warn">今日已满</Badge>}
        {b?.cycle_rolled && <Badge tone="warn">周期已滚动</Badge>}
        {b?.stale && <Badge tone="warn">核对已过期</Badge>}
        {b?.approximate && <Badge>余额为近似值</Badge>}
      </div>
      <Kv
        rows={[
          ["最近 1 小时", requestsText(a.w1h)],
          ["最近 24 小时", requestsText(a.w24h)],
          ["上游错误（24 小时）", upstreamErrorsText(a.w24h)],
          ["今日出图", `${a.images_today} 张`],
        ]}
      />
      {b ? (
        <Kv
          rows={[
            ["Anlas 余额", b.cycle_rolled ? "—" : `约 ${anlasText(b.balance)}（固定 ${anlasText(b.fixed)}）`],
            ["续费", b.expires_at ? `${fullTime(b.expires_at)}（剩 ${daysLeftText(b)}）` : "—"],
            ["日均用量", `${anlasText(Math.round(b.avg_daily))} Anlas（近 3 天）`],
            ["上次核对", ageText(b.snapshot_age_days)],
          ]}
        />
      ) : (
        <p class="muted small">还没有这个账号的核对记录，先在下方「Anlas 消耗核对」核对一次。</p>
      )}
      {b?.approximate && (
        <p class="muted small">
          另有约 {anlasText(Math.round(b.unattributed_anlas))} Anlas 的用量没有记录是哪个账号，没有算进来。
        </p>
      )}
    </div>
  );
}

export default function AccountsCard() {
  const { data, error, loading } = useAdmin<UpstreamStats>("/admin/api/upstream-stats");
  const stale = data?.accounts.some((a) => a.balance?.stale);
  return (
    <Card
      title="账号对比"
      sub={
        data
          ? `按上游账号分开统计 · 生成于 ${fullTime(data.generated_at)}。余额由上次核对减去之后的账本用量推算，只作参考。`
          : "按上游账号分开统计"
      }
    >
      {loading && !data && <Loading rows={2} />}
      {!data && error && <LoadError error={error} />}
      {stale && (
        <Notice tone="warn">距离上次真实核对已超过 3 天，余额推算可能偏离，请在下方重新核对。</Notice>
      )}
      {data && !data.accounts.length && <p class="muted small">未配置上游账号。</p>}
      <div class="token-grid">
        {data?.accounts.map((a) => (
          <Account key={a.token_id} a={a} />
        ))}
      </div>
    </Card>
  );
}
