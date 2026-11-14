/**
 * 设置：成员额度、领取条件、分享要求、全站预算、账号保护、维护与密码。
 *
 * 一页到底，不再拆成「成员规则 / 运行保护 / 系统」三个分区。
 * 每张卡片各自保存；数字字段的 0 一律是 0（无 / 不允许），「不限」填 9999。
 */
import { useMemo, useState } from "preact/hooks";
import { Button, Card } from "../../../ui";
import { act, useAdmin } from "../data";
import { CardPending, SectionHead, useBusy } from "./_kit";
import {
  type Settings as BudgetSettings,
  type Guard,
  guardSpecs,
  RUNTIME_NAMES,
  type RuntimeLimits,
  runtimeSpecs,
} from "./limits/core";
import {
  type MemberPolicy,
  type PolicyBound,
  PolicyFields,
  policyFromForm,
  policySpecs,
} from "./member-policy";
import { claimBody, type Ops, type RoleRule, validRole } from "./onboarding/core";
import { ShareLockCard } from "./share-lock";
import { NumField, SaveBar, Sect, SelectField, SwitchRow, useForm } from "./system/forms";
import PasswordCard from "./system/password";
import { MaintenanceCard } from "./system/server";

/* ------------------------------------------------------------ 成员额度 */

function DefaultRules() {
  const query = useAdmin<{ defaults: MemberPolicy; fields: Record<string, PolicyBound> }>(
    "/admin/api/member-rules",
  );
  const data = query.data;
  const server = useMemo(() => (data ? { ...data.defaults } : null), [data]);
  const specs = useMemo(() => (data ? policySpecs(data.fields) : {}), [data]);
  const form = useForm(specs, server);
  const [saving, setSaving] = useState(false);
  if (!data) return <CardPending title="成员额度" loading={query.loading} error={query.error} />;
  const save = async () => {
    setSaving(true);
    const ok = await act(
      "PUT",
      "/admin/api/member-rules",
      policyFromForm(data.defaults, form),
      "成员额度已保存",
    );
    setSaving(false);
    if (ok) form.commit();
  };
  return (
    <Card title="成员额度" sub="所有成员的默认额度；个人例外在成员页单独设置。0 = 停用，9999 = 不限。">
      <form {...form.formProps} onSubmit={(event) => event.preventDefault()}>
        <PolicyFields form={form} />
        <SaveBar form={form} saving={saving} onSave={save} />
      </form>
    </Card>
  );
}

/* ------------------------------------------------------------ 领取条件 */

function ClaimCard() {
  const query = useAdmin<Ops>("/admin/api/ops");
  if (!query.data) return <CardPending title="领取条件" loading={query.loading} error={query.error} />;
  return <ClaimForm ops={query.data} />;
}

export function ClaimForm({ ops }: { ops: Ops }) {
  const server = ops.registration;
  const bounds = server.fields;
  const form = useForm(
    {
      max_users: {
        kind: bounds.max_users.kind,
        label: "名额上限（0 = 不发放，9999 = 不限）",
        min: bounds.max_users.min,
        max: bounds.max_users.max,
        unit: "人",
      },
      expires_days: {
        kind: bounds.expires_days.kind,
        label: "Key 有效期（天，9999 = 永久）",
        min: bounds.expires_days.min,
        max: bounds.expires_days.max,
        unit: "天",
      },
    },
    { ...server },
  );
  const [draft, setDraft] = useState<{ roles: RoleRule[]; open: boolean } | null>(null);
  const [saved, setSaved] = useState<{ base: Ops["registration"]; roles: RoleRule[]; open: boolean } | null>(
    null,
  );
  const [saving, setSaving] = useState(false);
  const baseline = saved?.base === server ? saved : server;
  const roles = draft?.roles ?? baseline.roles;
  const open = draft?.open ?? baseline.open;
  const change = (nextRoles: RoleRule[], nextOpen = open) => setDraft({ roles: nextRoles, open: nextOpen });
  const dirty = form.dirty || JSON.stringify({ roles, open }) !== JSON.stringify(baseline);
  useBusy(dirty);
  const valid =
    form.valid &&
    roles.every(validRole) &&
    new Set(roles.map((role) => `${role.guild}/${role.role}`)).size === roles.length;
  const save = async () => {
    setSaving(true);
    const ok = await act(
      "PUT",
      "/admin/api/ops/registration",
      { ...claimBody(server), ...form.values(), open, roles },
      "领取条件已保存",
    );
    setSaving(false);
    if (ok) {
      form.commit();
      setSaved({ base: server, roles, open });
      setDraft(null);
    }
  };
  return (
    <Card title="领取条件" sub="Discord 自助领 Key 的门槛；保存后机器人下一条命令即生效。">
      <form
        {...form.formProps}
        onSubmit={(event) => {
          event.preventDefault();
          if (valid && dirty) void save();
        }}
      >
        <SwitchRow title="开放领取" checked={open} onChange={(value) => change(roles, value)} />
        <div class="form-grid">
          <NumField form={form} name="max_users" />
          <NumField form={form} name="expires_days" />
        </div>
        <div class="form-section">
          <div class="form-section__title">符合任一身份组（留空 = 本服务器成员）</div>
          {roles.map((role, index) => (
            <div class="mr-role" key={index}>
              {(["guild", "role", "note"] as const).map((name) => (
                <div class="field" key={name}>
                  <label class="field__label" for={`role-${index}-${name}`}>
                    {{ guild: "服务器 ID", role: "身份组 ID", note: "显示名" }[name]}
                  </label>
                  <input
                    id={`role-${index}-${name}`}
                    class="input"
                    value={role[name]}
                    maxLength={name === "note" ? 80 : 20}
                    inputMode={name === "note" ? "text" : "numeric"}
                    onInput={(event) =>
                      change(
                        roles.map((row, i) =>
                          i === index ? { ...row, [name]: event.currentTarget.value } : row,
                        ),
                      )
                    }
                  />
                </div>
              ))}
              <Button
                aria-label={`删除身份组条件 ${index + 1}`}
                onClick={() => change(roles.filter((_, i) => i !== index))}
              >
                删除
              </Button>
            </div>
          ))}
          <Button
            size="sm"
            disabled={roles.length >= bounds.roles.max}
            onClick={() => change([...roles, { guild: "", role: "", note: "" }])}
          >
            添加身份组
          </Button>
        </div>
        <div class="savebar">
          <Button variant="primary" type="submit" disabled={!dirty || !valid || saving}>
            {saving ? "保存中…" : "保存领取条件"}
          </Button>
        </div>
      </form>
    </Card>
  );
}

/* ------------------------------------------------------------ 全站预算 */

function BudgetCard() {
  const query = useAdmin<BudgetSettings>("/admin/api/settings");
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
  const fields = query.data?.fields;
  const specs = useMemo(() => {
    if (!fields) return {};
    return Object.fromEntries(
      [
        ["daily_v5", "V5 全站每天", "张"],
        ["daily_anlas", "Anlas 全站每天", "Anlas"],
        ["monthly_anlas", "Anlas 全站每月", "Anlas"],
      ].map(([name, label, unit]) => [
        name as string,
        {
          kind: fields[name as keyof typeof fields].kind,
          label: label as string,
          min: fields[name as keyof typeof fields].min,
          max: fields[name as keyof typeof fields].max,
          unit: unit as string,
        },
      ]),
    );
  }, [fields]);
  const form = useForm(specs, server);
  const [saving, setSaving] = useState(false);
  if (!query.data || !server)
    return <CardPending title="全站预算" loading={query.loading} error={query.error} />;
  const save = async () => {
    const value = { ...server, ...form.values() };
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
      "全站预算已保存",
    );
    setSaving(false);
    if (ok) form.commit();
  };
  return (
    <Card title="全站预算" sub="0 = 停用（谁也用不了），9999 = 不限。">
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

/* ------------------------------------------------------------ 账号保护 */

function GuardCard() {
  const guard = useAdmin<Guard>("/admin/api/guard");
  const runtime = useAdmin<RuntimeLimits>("/admin/api/runtime-limits");
  const specs = useMemo(
    () => (guard.data && runtime.data ? { ...runtimeSpecs(runtime.data), ...guardSpecs(guard.data) } : {}),
    [guard.data, runtime.data],
  );
  const server = useMemo(
    () =>
      guard.data && runtime.data
        ? {
            queue_timeout: runtime.data.queue_timeout,
            image_429_cooldown_seconds: runtime.data.image_429_cooldown_seconds,
            ...guard.data.values,
          }
        : null,
    [guard.data, runtime.data],
  );
  const form = useForm(specs, server);
  const [saving, setSaving] = useState(false);
  if (!guard.data || !runtime.data)
    return (
      <CardPending
        title="账号保护与排队"
        loading={guard.loading || runtime.loading}
        error={guard.error ?? runtime.error}
      />
    );
  const save = async () => {
    const run: Record<string, unknown> = {};
    const rest: Record<string, unknown> = {};
    for (const [k, v] of Object.entries(form.changes())) {
      (RUNTIME_NAMES.includes(k) ? run : rest)[k] = v;
    }
    setSaving(true);
    let ok = true;
    if (Object.keys(run).length)
      ok = (await act("PUT", "/admin/api/runtime-limits", run, "运行限制已保存")) !== null;
    if (ok && Object.keys(rest).length)
      ok = (await act("PUT", "/admin/api/guard", rest, "账号保护已保存")) !== null;
    setSaving(false);
    if (ok) form.commit();
  };
  return (
    <Card title="账号保护与排队" sub="官方账号的出图节奏；拼车上游用自己的等待设置。">
      <form {...form.formProps} onSubmit={(e) => e.preventDefault()}>
        <div class="form-grid">
          {Object.keys(specs).map((name) => (
            <NumField key={name} form={form} name={name} />
          ))}
        </div>
        <SaveBar form={form} onSave={save} saving={saving} label="保存账号保护" />
      </form>
    </Card>
  );
}

/* ------------------------------------------------------------ 页面 */

export default function Settings() {
  return (
    <Sect>
      <SectionHead title="设置" desc="每张卡片各自保存；只提交改过的字段。" />
      <DefaultRules />
      <ClaimCard />
      <ShareLockCard />
      <BudgetCard />
      <GuardCard />
      <MaintenanceCard />
      <PasswordCard />
    </Sect>
  );
}
