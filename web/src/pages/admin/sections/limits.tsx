import { useMemo, useState } from "preact/hooks";
import { fullTime, withUnit } from "../../../lib/format";
import { Badge, Card } from "../../../ui";
import { act, useAdmin } from "../Admin";
import { CardPending, SectionHead, TableWrap } from "./_kit";
import {
  agoText,
  dash,
  type Guard,
  guardSpecs,
  type Records,
  RUNTIME_SPECS,
  type Settings,
} from "./limits/core";
import type { Ops } from "./onboarding/core";
import { FeaturesCard } from "./onboarding/features";
import { NumField, SaveBar, Sect, SelectField, SwitchRow, useForm } from "./system/forms";

function BudgetCard() {
  const query = useAdmin<Settings>("/admin/api/settings");
  const server = useMemo(
    () =>
      query.data
        ? {
            ...query.data.budgets,
            v5_paused: query.data.v5_paused,
            v5_medium_mode: query.data.v5_medium_mode,
          }
        : null,
    [query.data],
  );
  const form = useForm(
    {
      daily_v5: { kind: "int", label: "V5 全站每天", min: 0, max: 9999, unit: "张" },
      daily_anlas: { kind: "int", label: "Anlas 全站每天", min: 0, max: 9999, unit: "Anlas" },
      monthly_anlas: { kind: "float", label: "Anlas 全站每月", min: 0, max: 99999, unit: "Anlas" },
      v5_paused: { kind: "bool", label: "暂停 V5" },
      v5_medium_mode: {
        kind: "select",
        label: "V5 模型",
        options: [
          { value: "off", label: "保留请求模型" },
          { value: "on", label: "Full 转为 Medium" },
        ],
      },
    },
    server,
  );
  const [saving, setSaving] = useState(false);
  if (!query.data || !server)
    return <CardPending title="全站预算（0 停用）" loading={query.loading} error={query.error} />;
  const save = async () => {
    const value = { ...server, ...form.changes() };
    setSaving(true);
    const ok = await act(
      "PUT",
      "/admin/api/settings",
      {
        budgets: {
          daily_v5: value.daily_v5,
          daily_anlas: value.daily_anlas,
          monthly_anlas: value.monthly_anlas,
        },
        v5_paused: value.v5_paused,
        v5_medium_mode: value.v5_medium_mode,
      },
      "运行保护已保存",
    );
    setSaving(false);
    if (ok) form.commit();
  };
  return (
    <Card title="全站预算（0 停用）">
      <form {...form.formProps} onSubmit={(event) => event.preventDefault()}>
        <div class="form-grid">
          {["daily_v5", "daily_anlas", "monthly_anlas"].map((name) => (
            <NumField key={name} form={form} name={name} />
          ))}
        </div>
        <SwitchRow
          title="暂停 V5"
          checked={form.raw("v5_paused") === true}
          onChange={(value) => form.set("v5_paused", value)}
        />
        <SelectField form={form} name="v5_medium_mode" />
        <SaveBar form={form} saving={saving} onSave={save} />
      </form>
    </Card>
  );
}

/* ------------------------------------------------- 账号保护与节奏（手动） */

/** 保护字段按用途分组；接口以后加了新字段也能落到「其他」组里，不会丢。 */
const GUARD_GROUPS: Array<{ title: string; names: string[] }> = [
  { title: "出图节奏", names: ["account_daily_cap", "post_jitter_min", "post_jitter_max"] },
  { title: "排队", names: ["queue_per_member", "queue_per_account", "queue_timeout"] },
  { title: "故障冷却", names: ["image_429_cooldown_seconds"] },
];

function GuardCard() {
  const guard = useAdmin<Guard>("/admin/api/guard");
  const runtime = useAdmin<Record<string, number>>("/admin/api/runtime-limits");
  const specs = useMemo(
    () => (guard.data ? { ...RUNTIME_SPECS, ...guardSpecs(guard.data) } : RUNTIME_SPECS),
    [guard.data],
  );
  const server = useMemo(
    () => (guard.data && runtime.data ? { ...runtime.data, ...guard.data.values } : null),
    [guard.data, runtime.data],
  );
  const form = useForm(specs, server);
  const [saving, setSaving] = useState(false);
  if (!guard.data || !runtime.data)
    return (
      <CardPending
        title="账号保护与节奏"
        loading={guard.loading || runtime.loading}
        error={guard.error ?? runtime.error}
      />
    );
  const save = async () => {
    const ch = form.changes();
    const run: Record<string, unknown> = {};
    const g: Record<string, unknown> = {};
    for (const [k, v] of Object.entries(ch)) (k in RUNTIME_SPECS ? run : g)[k] = v;
    setSaving(true);
    let ok = true;
    if (Object.keys(run).length)
      ok = (await act("PUT", "/admin/api/runtime-limits", run, "运行限制已保存并生效")) !== null && ok;
    if (ok && Object.keys(g).length)
      ok = (await act("PUT", "/admin/api/guard", g, "账号保护已保存，立即生效")) !== null;
    setSaving(false);
    if (ok) form.commit();
  };
  const names = Object.keys(specs);
  const grouped = new Set(GUARD_GROUPS.flatMap((g) => g.names));
  const rest = names.filter((n) => !grouped.has(n));
  return (
    <Card title="账号保护与节奏">
      <form {...form.formProps} onSubmit={(e) => e.preventDefault()}>
        {GUARD_GROUPS.map((group) => (
          <div class="form-section" key={group.title}>
            <div class="form-section__title">{group.title}</div>
            <div class="form-grid">
              {group.names
                .filter((n) => n in specs)
                .map((n) => (
                  <NumField key={n} form={form} name={n} />
                ))}
            </div>
          </div>
        ))}
        {rest.length > 0 && (
          <div class="form-section">
            <div class="form-section__title">其他</div>
            <div class="form-grid">
              {rest.map((n) => (
                <NumField key={n} form={form} name={n} />
              ))}
            </div>
          </div>
        )}
        <SaveBar form={form} onSave={save} saving={saving} label="保存账号保护" />
      </form>
    </Card>
  );
}

/* ------------------------------------------------- 上游账号现状（只显示） */

/** 保护状态（/admin/api/guard）和电量记录（/admin/api/quota-records）按账号合成一张表。 */
function AccountsCard() {
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
  const rows = (guard.data.accounts ?? []).map((a) => ({ a, r: rec.get(a.position) }));
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
              <tr key={a.position}>
                <td>#{a.position}</td>
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
                  {withUnit(a.today, "张")} / {r ? r.today.v5 : "—"}
                </td>
                <td>{r ? `${r.yesterday.images} / ${r.yesterday.v5}` : "—"}</td>
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

export default function Limits() {
  const ops = useAdmin<Ops>("/admin/api/ops");
  return (
    <Sect>
      <SectionHead title="运行保护" />
      <BudgetCard />
      {ops.data && <FeaturesCard ops={ops.data} />}
      <GuardCard />
      <AccountsCard />
    </Sect>
  );
}
