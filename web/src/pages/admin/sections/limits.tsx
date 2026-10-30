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

export default function Limits() {
  const ops = useAdmin<Ops>("/admin/api/ops");
  return (
    <Sect>
      <SectionHead title="运行保护" />
      <BudgetCard />
      {ops.data && <FeaturesCard ops={ops.data} />}
      <GuardCard />
    </Sect>
  );
}
