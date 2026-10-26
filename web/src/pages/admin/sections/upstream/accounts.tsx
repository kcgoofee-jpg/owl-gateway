/**
 * 一行账号一张卡，按「这把 API Key 能告诉我们什么」分三段：
 * - 官方读数（/user/subscription + /user/information 的缓存）：V5 余量、订阅、Anlas、封禁；
 * - 核对推算（对账快照 + 本站账本）：Anlas 余额、续费倒计时；
 * - 本站观测（今天张数、请求窗口、在途）与操作（启停、换 Token、删除、V5 日限）。
 */
import { Ellipsis } from "lucide-preact";
import { useState } from "preact/hooks";
import { fullTime, usedOf, withUnit } from "../../../../lib/format";
import { Badge, Button, Card, Menu, Meter, Notice } from "../../../../ui";
import { act } from "../../Admin";
import { Kv, NumField, SaveBar, useForm, withTone } from "../system/forms";
import {
  ageText,
  anlasText,
  daysLeftText,
  requestsText,
  TOKEN_SPECS,
  tokenState,
  type UpstreamAccount,
  upstreamErrorsText,
} from "./core";

function OfficialReadings({ a }: { a: UpstreamAccount }) {
  const v5 = a.v5;
  const exhausted = v5?.is_negative === true;
  const tone = exhausted ? "danger" : v5?.low ? "warn" : undefined;
  const info = a.info;
  return (
    <div class="stack-sm">
      <div class="row">
        <b class="small">官方读数</b>
        {v5?.checked_at && <span class="muted small">更新于 {fullTime(v5.checked_at)}</span>}
        {v5?.uncertain && <Badge>{v5.stale ? "状态陈旧" : "状态不确定"}</Badge>}
        {info?.stale == null ? null : info.last_success == null ? (
          <Badge>未查询过账号状态</Badge>
        ) : (
          info.stale && <Badge>状态陈旧</Badge>
        )}
      </div>
      {!v5 || v5.percent == null ? (
        <p class="muted small">还没有 V5 读数，首次 V5 生图或手动查询后会显示。</p>
      ) : (
        <Meter
          label="V5 剩余"
          value={exhausted ? 0 : v5.percent}
          max={100}
          text={exhausted ? "已耗尽" : `${v5.percent}%`}
          {...withTone(tone)}
        />
      )}
      <p class="muted small">
        订阅：{v5?.active == null ? "未知" : v5.active ? "有效" : "无效"}
        {v5?.tier != null && ` · tier ${v5.tier}`}
        {v5?.expires_at ? ` · 到期 ${fullTime(v5.expires_at)}` : " · 到期未知"}
        {v5?.is_grace_period ? " · 宽限期" : ""}
        {v5 &&
          v5.fixed_anlas != null &&
          ` · 官方 Anlas ${anlasText(v5.fixed_anlas + (v5.purchased_anlas ?? 0))}`}
      </p>
      <p class="muted small">
        封禁：
        {info?.ban_status == null ? "未知" : info.ban_status === "not_banned" ? "未封禁" : info.ban_status}
        {info?.ban_message && `（${info.ban_message}）`} · 邮箱验证：
        {info?.email_verified == null ? "未知" : info.email_verified ? "已验证" : "未验证"}
        {info?.trial_activated != null && ` · 试用${info.trial_activated ? "已启用" : "未启用"}`}
      </p>
      {v5?.error && <Notice tone="warn">{v5.error}：V5 生图需确认后继续。</Notice>}
      {info?.last_error && (
        <Notice tone="warn">
          上次查询失败：{info.last_error}
          {info.last_error_at != null && `（${fullTime(info.last_error_at)}）`}；保留最近一次成功读数。
        </Notice>
      )}
    </div>
  );
}

function LedgerReadings({ a }: { a: UpstreamAccount }) {
  const b = a.balance;
  if (!b) {
    return (
      <div class="stack-sm">
        <b class="small">核对推算</b>
        <p class="muted small">还没有核对记录，先在下方「Anlas 消耗核对」核对一次。</p>
      </div>
    );
  }
  return (
    <div class="stack-sm">
      <div class="row">
        <b class="small">核对推算</b>
        {b.cycle_rolled && <Badge tone="warn">周期已滚动</Badge>}
        {b.stale && <Badge tone="warn">核对已过期</Badge>}
        {b.approximate && <Badge>余额为近似值</Badge>}
      </div>
      <p class="muted small">
        约 {anlasText(b.balance)} Anlas（固定 {anlasText(b.fixed)}） · 日均{" "}
        {anlasText(Math.round(b.avg_daily))}（近 3 天）
        {b.expires_at ? ` · 到期 ${fullTime(b.expires_at)}（剩 ${daysLeftText(b)}）` : ""} · 上次核对{" "}
        {ageText(b.snapshot_age_days)}
      </p>
      <p class="muted small">
        推算按上次核对减去之后的账本用量，只作参考；与官方读数不一致时以核对为准。
        {b.approximate && ` 另有约 ${anlasText(Math.round(b.unattributed_anlas))} Anlas 没有记录归属。`}
      </p>
    </div>
  );
}

export function AccountCard(props: {
  a: UpstreamAccount;
  only: boolean;
  onReplace: (a: UpstreamAccount) => void;
  onDelete: (a: UpstreamAccount) => void;
  onToggle: (a: UpstreamAccount) => void;
}) {
  const a = props.a;
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
  return (
    <Card>
      <header class="token__head">
        <span class="token__name mono">
          #{a.position} {a.token}
        </span>
        <Badge {...withTone(st.tone)} dot>
          {st.text}
        </Badge>
        <span class="spacer" />
        <Menu
          button={(b) => (
            <Button variant="ghost" icon aria-label={`账号 ${a.position} 的更多操作`} {...b}>
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
      <div class="token-grid">
        <OfficialReadings a={a} />
        <LedgerReadings a={a} />
      </div>
      <Kv
        rows={[
          ["今日生成", `${withUnit(a.images_today, "张")} · 失败 ${a.fails} 次`],
          ["免费 V5", `今日已用 ${usedOf(a.v5_used, a.v5_daily_limit)}`],
          ["在途", withUnit(a.image_active, "张")],
          ["Anlas", a.allow_anlas ? "允许消耗" : "禁止消耗"],
          ["最近 1 小时", requestsText(a.w1h)],
          ["最近 24 小时", `${requestsText(a.w24h)}；上游 ${upstreamErrorsText(a.w24h)}`],
        ]}
      />
      <form class="token__form" {...form.formProps} onSubmit={(e) => e.preventDefault()}>
        <div class="form-grid">
          <NumField form={form} name="v5_daily_limit" />
        </div>
        <SaveBar form={form} onSave={save} saving={saving} />
      </form>
    </Card>
  );
}
