/** Per-account official readings, ledger estimates and local request statistics. */
import { Ellipsis } from "lucide-preact";
import type { ComponentChildren } from "preact";
import { useState } from "preact/hooks";
import { dayOf, fullTime, shortTime } from "../../../../lib/format";
import { Badge, Button, Card, Menu, Meter, Notice } from "../../../../ui";
import { act } from "../../Admin";
import { NumField, SaveBar, useForm, withTone } from "../system/forms";
import {
  ageText,
  anlasText,
  answer,
  daysLeftText,
  duration,
  projectionText,
  reading,
  TOKEN_SPECS,
  tokenState,
  type UpstreamAccount,
  type WindowStats,
} from "./core";

type Fact = [string, ComponentChildren];
const accountTime = (ts: number | null | undefined): string => (ts ? `${dayOf(ts)} ${shortTime(ts)}` : "—");
function Facts({ rows }: { rows: Fact[] }) {
  return (
    <dl class="up-facts">
      {rows.map(([label, value]) => (
        <div key={label}>
          <dt>{label}</dt>
          <dd>{value}</dd>
        </div>
      ))}
    </dl>
  );
}
function Block({
  title,
  updated,
  children,
}: {
  title: string;
  updated?: string;
  children: ComponentChildren;
}) {
  return (
    <section class="up-block" aria-label={title}>
      <header class="up-block__head">
        <h3>{title}</h3>
        {updated && <time>{updated}</time>}
      </header>
      {children}
    </section>
  );
}
function RequestStats({ a }: { a: UpstreamAccount }) {
  const rows: Array<[string, (w: WindowStats) => string]> = [
    ["成功", (w) => reading(w.ok)],
    ["失败", (w) => reading(w.error)],
    ["选号后拒绝", (w) => reading(w.rejected)],
    ["401 / 403 / 429", (w) => `${w.up_401} / ${w.up_403} / ${w.up_429}`],
    ["耗时样本", (w) => reading(w.timing_samples)],
    ["平均排队", (w) => duration(w.avg_wait_ms == null ? null : w.avg_wait_ms / 1000)],
    ["P90 排队", (w) => duration(w.p90_wait_ms == null ? null : w.p90_wait_ms / 1000)],
    ["平均生成", (w) => duration(w.avg_dur_ms == null ? null : w.avg_dur_ms / 1000)],
  ];
  return (
    <table class="up-stats">
      <caption class="sr-only">账号 {a.position} 请求统计</caption>
      <thead>
        <tr>
          <th scope="col">请求统计</th>
          <th scope="col">1 小时</th>
          <th scope="col">24 小时</th>
        </tr>
      </thead>
      <tbody>
        {rows.map(([label, value]) => (
          <tr key={label}>
            <th scope="row">{label}</th>
            <td>{value(a.w1h)}</td>
            <td>{value(a.w24h)}</td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

export function AccountCard(props: {
  a: UpstreamAccount;
  only: boolean;
  onDetails: (a: UpstreamAccount) => void;
  onReplace: (a: UpstreamAccount) => void;
  onDelete: (a: UpstreamAccount) => void;
  onToggle: (a: UpstreamAccount) => void;
}) {
  const a = props.a;
  const v = a.v5;
  const info = a.info;
  const b = a.balance;
  const st = tokenState(a);
  const form = useForm(TOKEN_SPECS, a as unknown as Record<string, unknown>);
  const [saving, setSaving] = useState(false);
  const base = `/admin/api/upstream-tokens/${encodeURIComponent(a.token_id)}`;
  const save = async () => {
    const ch = form.changes();
    setSaving(true);
    let ok = true;
    if ("v5_daily_limit" in ch)
      ok =
        (await act("PUT", `${base}/v5-limit`, { v5_daily_limit: ch.v5_daily_limit }, "V5 日限已保存")) !==
        null;
    setSaving(false);
    if (ok) form.commit();
  };
  const officialTotal =
    v?.fixed_anlas != null && v.purchased_anlas != null ? v.fixed_anlas + v.purchased_anlas : null;
  return (
    <Card class="up-account">
      <header class="token__head">
        <h2 class="up-account__title">账号 {a.position}</h2>
        <span class="token__name mono">{a.token}</span>
        <Badge {...withTone(st.tone)} dot>
          {st.text}
        </Badge>
        <span class="spacer" />
        <Button onClick={() => props.onDetails(a)}>完整信息</Button>
        <Menu
          button={(button) => (
            <Button variant="ghost" icon aria-label={`账号 ${a.position} 的更多操作`} {...button}>
              <Ellipsis size={18} aria-hidden="true" />
            </Button>
          )}
          items={[
            { label: "替换 Token", onSelect: () => props.onReplace(a) },
            {
              label: a.admin_enabled ? "停用这个账号" : "启用这个账号",
              onSelect: () => props.onToggle(a),
              danger: a.admin_enabled,
            },
            ...(props.only ? [] : [{ label: "删除", onSelect: () => props.onDelete(a), danger: true }]),
          ]}
        />
      </header>
      <div class="up-highlights">
        <div>
          <span>V5 电量</span>
          <strong>{v?.percent == null ? "—" : `${v.percent}%`}</strong>
          <Meter
            label={<span class="sr-only">V5 电量</span>}
            value={Math.max(0, Math.min(100, v?.percent ?? 0))}
            max={100}
            text={answer(v?.is_negative, "负额度", v?.low ? "余量低" : "")}
            {...withTone(v?.low ? "warn" : undefined)}
          />
        </div>
        <div>
          <span>官方 Anlas</span>
          <strong>{anlasText(officialTotal)}</strong>
          <small>
            <span>固定 {anlasText(v?.fixed_anlas)}</span>
            <span>购买 {anlasText(v?.purchased_anlas)}</span>
          </small>
        </div>
        <div>
          <span>今日出图</span>
          <strong>{reading(a.images_today)}</strong>
          <small>
            免费 V5 {reading(a.v5_used)} / {reading(a.v5_daily_limit)}
          </small>
        </div>
      </div>
      <div class="up-sections">
        <Block title="订阅与账号" updated={fullTime(info?.last_success)}>
          <Facts
            rows={[
              ["订阅", answer(v?.active, "有效", "无效")],
              ["订阅等级", reading(v?.tier)],
              ["订阅到期", accountTime(v?.expires_at)],
              ["宽限期", answer(v?.is_grace_period, "是", "否")],
              ["账号类型", reading(v?.account_type)],
              ["账号创建", accountTime(info?.account_created_at)],
              ["封禁状态", info?.ban_status === "not_banned" ? "未封禁" : (info?.ban_status ?? "—")],
              ["邮箱验证", answer(info?.email_verified, "已验证", "未验证")],
              ["试用", answer(info?.trial_activated, "已启用", "未启用")],
              ["试用操作", reading(info?.trial_actions_left)],
              ["试用图片", reading(info?.trial_images_left)],
              ["状态读数", info?.last_success == null ? "未查询" : info.stale ? "已陈旧" : "已更新"],
            ]}
          />
          {info?.ban_message && <p class="up-message">{info.ban_message}</p>}
          {info?.last_error && (
            <Notice tone="warn">
              {info.last_error} · {fullTime(info.last_error_at)}
            </Notice>
          )}
        </Block>
        <Block title="电量与恢复" updated={fullTime(v?.checked_at)}>
          <Facts
            rows={[
              ["负额度", answer(v?.is_negative, "是", "否")],
              ["额外购买电量", reading(v?.purchased_percent, "%")],
              ["下次恢复", duration(v?.time_until_next_percent_seconds)],
              ["当前推算恢复", reading(v?.current_response_recharge_per_day_estimate, "% / 天")],
              ["历史推算恢复", reading(v?.historical_sample_based_recharge_per_day_estimate, "% / 天")],
              [
                "电量读数",
                v?.checked_at == null ? "未查询" : v.stale ? "已陈旧" : v.uncertain ? "不确定" : "已更新",
              ],
              ["负载占比 · 24h", reading(a.load_share_24h, "%")],
              ["推算余额占比", reading(a.balance_share, "%")],
            ]}
          />
          {v?.error && <Notice tone="warn">{v.error}</Notice>}
        </Block>
        <Block title="余额推算" updated={b ? ageText(b.snapshot_age_days) : "未核对"}>
          <Facts
            rows={[
              ["推算总额", anlasText(b?.balance)],
              ["固定 Anlas", anlasText(b?.fixed)],
              ["购买 Anlas", anlasText(b?.purchased)],
              ["日均 · 近 3 天", b ? reading(b.avg_daily, " Anlas") : "—"],
              ["订阅剩余", b ? daysLeftText(b) : "—"],
              ["核对到期日", accountTime(b?.expires_at)],
              ["到期预测", projectionText(b)],
              ["未归属消耗", anlasText(b?.unattributed_anlas)],
              [
                "推算状态",
                !b
                  ? "未核对"
                  : [b.cycle_rolled && "周期已滚动", b.stale && "已陈旧", b.approximate && "近似值"]
                      .filter(Boolean)
                      .join(" · ") || "已更新",
              ],
            ]}
          />
        </Block>
        <Block title="请求与运行">
          <RequestStats a={a} />
          <Facts
            rows={[
              ["在途 / 并发", `${a.image_active} / ${a.image_concurrency}`],
              ["连续失败", reading(a.fails)],
              ["冷却剩余", duration(a.blocked_for)],
              ["最近成功", fullTime(a.last_ok)],
              ["账号可用", answer(a.usable, "是", "否")],
              ["Anlas 消耗", a.allow_anlas ? "允许" : "禁止"],
            ]}
          />
        </Block>
      </div>
      <form class="token__form" {...form.formProps} onSubmit={(e) => e.preventDefault()}>
        <NumField form={form} name="v5_daily_limit" />
        <SaveBar form={form} onSave={save} saving={saving} />
      </form>
    </Card>
  );
}
