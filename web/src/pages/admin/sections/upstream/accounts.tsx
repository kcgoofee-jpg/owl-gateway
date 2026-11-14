/** 官方账号卡片：三个关键读数 + 一行运行统计。长篇明细统一放在「完整信息」对话框里。 */
import { Ellipsis } from "lucide-preact";
import { useState } from "preact/hooks";
import { dayOf, fullTime, shortTime } from "../../../../lib/format";
import { Badge, Button, Card, Menu, Meter, Notice } from "../../../../ui";
import { act } from "../../data";
import { NumField, SaveBar, useForm, withTone } from "../system/forms";
import {
  anlasText,
  duration,
  limitAgo,
  queueText,
  quietText,
  reading,
  tokenState,
  type UpstreamAccount,
  v5Highlight,
  v5LimitSpecs,
  type WindowStats,
  yesterdayText,
} from "./core";

const accountTime = (ts: number | null | undefined): string => (ts ? `${dayOf(ts)} ${shortTime(ts)}` : "—");

/** 1 小时 / 24 小时的成败一眼看完，不再铺一张 8 行 3 列的表。 */
function windowText(w: WindowStats): string {
  return `成功 ${reading(w.ok)} · 失败 ${reading(w.error)} · 429 ${reading(w.up_429)}`;
}

export function RequestStats({ a }: { a: Pick<UpstreamAccount, "position" | "w1h" | "w24h"> }) {
  return (
    <dl class="up-facts">
      <div>
        <dt>1 小时</dt>
        <dd>{windowText(a.w1h)}</dd>
      </div>
      <div>
        <dt>24 小时</dt>
        <dd>{windowText(a.w24h)}</dd>
      </div>
    </dl>
  );
}

export function AccountCard(props: {
  a: UpstreamAccount;
  limit: { min: number; max: number };
  only: boolean;
  onDetails: (a: UpstreamAccount) => void;
  onReplace: (a: UpstreamAccount) => void;
  onDelete: (a: UpstreamAccount) => void;
  onToggle: (a: UpstreamAccount) => void;
}) {
  const a = props.a;
  const v = a.v5;
  const b = a.balance;
  const info = a.info;
  const st = tokenState(a);
  const form = useForm(v5LimitSpecs(props.limit), a as unknown as Record<string, unknown>);
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
  const meter = v5Highlight(v);
  return (
    <Card class="up-account">
      <header class="token__head">
        <h2 class="up-account__title">账号 {a.position}</h2>
        <span class="token__name mono">{a.token}</span>
        {a.donated && <Badge tone="info">捐赠</Badge>}
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
          <strong>{meter.label}</strong>
          {meter.images != null ? <small>约 {reading(meter.images)} 张（官网 23 步估）</small> : null}
          <Meter
            label={<span class="sr-only">V5 电量</span>}
            value={meter.value}
            max={100}
            text={meter.status}
            {...withTone(meter.warn ? "warn" : undefined)}
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

      {a.manual_cap && (
        <Notice tone="warn">
          临时每天上限 {reading(a.manual_cap.cap)} 张，到 {fullTime(a.manual_cap.until)}
        </Notice>
      )}
      {a.paid_v5 && (
        <Notice tone="warn">
          站付 V5 已用 {reading(Math.round(a.paid_v5.spent))} / {reading(Math.round(a.paid_v5.max_anlas))}{" "}
          Anlas，到 {fullTime(a.paid_v5.until)}
        </Notice>
      )}
      {info?.ban_message && <Notice tone="warn">{info.ban_message}</Notice>}
      {info?.last_error && (
        <Notice tone="warn">
          {info.last_error} · {fullTime(info.last_error_at)}
        </Notice>
      )}
      {v?.error && <Notice tone="warn">{v.error}</Notice>}

      <dl class="up-facts">
        <div>
          <dt>昨天</dt>
          <dd>{yesterdayText(a.yesterday?.images, a.yesterday?.v5)}</dd>
        </div>
        <div>
          <dt>排队</dt>
          <dd>{queueText(a.waiting, a.queue_cap)}</dd>
        </div>
        <div>
          <dt>在途 / 并发</dt>
          <dd>
            {a.image_active} / {a.image_concurrency}
          </dd>
        </div>
        <div>
          <dt>出图间隔</dt>
          <dd>{a.cycle ? duration(a.cycle) : "—"}</dd>
        </div>
        <div>
          <dt>最近限流</dt>
          <dd>{limitAgo(a.last_429)}</dd>
        </div>
        <div>
          <dt>连续失败</dt>
          <dd>{reading(a.fails)}</dd>
        </div>
        <div>
          <dt>冷却剩余</dt>
          <dd>{duration(a.blocked_for)}</dd>
        </div>
        <div>
          <dt>最近成功</dt>
          <dd>{a.last_ok ? accountTime(a.last_ok) : "—"}</dd>
        </div>
        <div>
          <dt>Anlas</dt>
          <dd>{a.allow_anlas ? "允许消耗" : "禁止消耗"}</dd>
        </div>
        <div>
          <dt>恢复估算</dt>
          <dd>{quietText(a.quiet_rate?.rate)}</dd>
        </div>
        <div>
          <dt>余额读数</dt>
          <dd>{b ? `${anlasText(b.balance)} · ${b.stale ? "已陈旧" : "最新"}` : "还没有快照"}</dd>
        </div>
        <div>
          <dt>1 小时</dt>
          <dd>{windowText(a.w1h)}</dd>
        </div>
        <div>
          <dt>24 小时</dt>
          <dd>{windowText(a.w24h)}</dd>
        </div>
      </dl>

      <form class="token__form" {...form.formProps} onSubmit={(e) => e.preventDefault()}>
        <NumField form={form} name="v5_daily_limit" />
        <SaveBar form={form} onSave={save} saving={saving} />
      </form>
    </Card>
  );
}
