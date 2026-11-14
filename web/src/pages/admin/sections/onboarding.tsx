import { useMemo, useState } from "preact/hooks";
import { Button, Card } from "../../../ui";
import { act, useAdmin } from "../data";
import { CardPending, SectionHead, useBusy } from "./_kit";
import {
  type MemberPolicy,
  type PolicyBound,
  PolicyFields,
  policyFromForm,
  policySpecs,
} from "./member-policy";
import { claimBody, type Ops, type RoleRule, validRole } from "./onboarding/core";
import { ShareLockCard } from "./share-lock";
import { SaveBar, Sect, SwitchRow, useForm, ZeroModeField } from "./system/forms";

function DefaultRules() {
  const query = useAdmin<{ defaults: MemberPolicy; fields: Record<string, PolicyBound> }>(
    "/admin/api/member-rules",
  );
  const data = query.data;
  const server = useMemo(() => (data ? { ...data.defaults } : null), [data]);
  const specs = useMemo(() => (data ? policySpecs(data.fields) : {}), [data]);
  const form = useForm(specs, server);
  const [saving, setSaving] = useState(false);
  if (!data) return <CardPending title="默认规则" loading={query.loading} error={query.error} />;
  const save = async () => {
    setSaving(true);
    const ok = await act(
      "PUT",
      "/admin/api/member-rules",
      policyFromForm(data.defaults, form),
      "默认规则已保存",
    );
    setSaving(false);
    if (ok) form.commit();
  };
  return (
    <Card title="默认规则">
      <form {...form.formProps} onSubmit={(event) => event.preventDefault()}>
        <PolicyFields form={form} />
        <SaveBar form={form} saving={saving} onSave={save} />
      </form>
    </Card>
  );
}
export function ClaimForm({ ops }: { ops: Ops }) {
  const server = ops.registration;
  const record = useMemo(() => ({ ...server }), [server]);
  const bounds = server.fields;
  const form = useForm(
    {
      max_users: {
        kind: bounds.max_users.kind,
        label: "名额上限（0 不限）",
        min: bounds.max_users.min,
        max: bounds.max_users.max,
        unit: "人",
      },
      expires_days: {
        kind: bounds.expires_days.kind,
        label: "Key 有效期（0 永久）",
        min: bounds.expires_days.min,
        max: bounds.expires_days.max,
        unit: "天",
      },
    },
    record,
  );
  const [draft, setDraft] = useState<{ roles: RoleRule[]; open: boolean } | null>(null);
  const [confirmed, setConfirmed] = useState<{
    base: Ops["registration"];
    roles: RoleRule[];
    open: boolean;
  } | null>(null);
  const [saving, setSaving] = useState(false);
  const baseline = confirmed?.base === server ? confirmed : server;
  const roles = draft?.roles ?? baseline.roles;
  const open = draft?.open ?? baseline.open;
  const change = (roles: RoleRule[], openValue = open) => setDraft({ roles, open: openValue });
  const dirty =
    form.dirty ||
    (draft !== null &&
      JSON.stringify({ roles, open }) !== JSON.stringify({ roles: baseline.roles, open: baseline.open }));
  useBusy(dirty);
  const valid =
    form.valid &&
    roles.every(validRole) &&
    new Set(roles.map((role) => `${role.guild}/${role.role}`)).size === roles.length;
  const save = async () => {
    setSaving(true);
    const body = { ...claimBody(server), ...form.values(), open, roles };
    const ok = await act("PUT", "/admin/api/ops/registration", body, "领取条件已保存");
    setSaving(false);
    if (ok) {
      form.commit();
      setConfirmed({ base: server, roles, open });
      setDraft((current) =>
        current && JSON.stringify(current) === JSON.stringify({ roles, open }) ? null : current,
      );
    }
  };
  return (
    <Card title="领取条件">
      <form
        {...form.formProps}
        onSubmit={(event) => {
          event.preventDefault();
          if (valid && dirty) void save();
        }}
      >
        <SwitchRow title="开放领取" checked={open} onChange={(value) => change(roles, value)} />
        <div class="form-grid">
          <ZeroModeField form={form} name="max_users" zeroLabel="不限人数" />
          <ZeroModeField form={form} name="expires_days" zeroLabel="永久有效" />
        </div>
        <div class="form-section">
          <div class="form-section__title">符合任一身份组</div>
          {roles.length === 0 && <p class="muted">本服务器成员</p>}
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
export default function Onboarding() {
  const ops = useAdmin<Ops>("/admin/api/ops");
  return (
    <Sect>
      <SectionHead title="成员规则" />
      <DefaultRules />
      <ShareLockCard />
      {ops.data ? (
        <ClaimForm ops={ops.data} />
      ) : (
        <CardPending title="领取条件" loading={ops.loading} error={ops.error} />
      )}
    </Sect>
  );
}
