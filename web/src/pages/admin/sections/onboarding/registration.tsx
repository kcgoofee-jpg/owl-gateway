/** 自助领 Key：开放开关、名额、有效期、新成员默认额度与功能、限定身份组。 */
import { useMemo, useState } from "preact/hooks";
import { Badge, Button, Card, Meter, Notice, toast } from "../../../../ui";
import { actOrError, ConfirmDialog } from "../_kit";
import { NumField, SaveBar, SelectField, SwitchRow, TextField, useForm, withTone } from "../system/forms";
import { FEATURE_PREFIX, type Ops, regBody, regServer, regSpecs, roleProblem } from "./core";
import { ExtraRoles } from "./ExtraRoles";

const URL = "/admin/api/ops/registration";

export default function RegistrationCard(props: { ops: Ops }) {
  const r = props.ops.registration;
  const labels = props.ops.features.labels;
  const names = useMemo(() => Object.keys(labels), [labels]);
  const specs = useMemo(() => regSpecs(labels), [labels]);
  const server = useMemo(() => regServer(r, labels), [r, labels]);
  const form = useForm(specs, server);
  const [saving, setSaving] = useState(false);
  const [toggleTo, setToggleTo] = useState<boolean | null>(null);

  const roleErr = roleProblem(String(form.raw("role_guild")), String(form.raw("role_id")));
  const pct = r.max_users > 0 ? Math.min(100, (r.active / r.max_users) * 100) : 0;
  const customFeatures = form.raw("features_mode") === "custom";

  /** 保存就是保存（发成员公告 10-23 删除） */
  const save = async () => {
    if (roleErr) {
      toast(roleErr, { error: true });
      return;
    }
    const body = regBody(form.changes(), form.raw as (n: string) => string | boolean, names);
    setSaving(true);
    const err = await actOrError("PUT", URL, body, "领 Key 设置已保存");
    setSaving(false);
    if (err) toast(err, { error: true });
    else form.commit();
  };
  /** 返回 undefined = 已改；返回字符串 = 失败原因 */
  const setOpen = (open: boolean) => actOrError("PUT", URL, { open }, open ? "已开放领 Key" : "已暂停领 Key");

  return (
    <Card
      title="自助领 Key"
      sub="Discord /register 自助领 Key 的开放状态、名额限制与新成员默认权限。机器人下一条命令即生效。"
      actions={
        <Badge {...withTone(r.open ? "ok" : "warn")} dot>
          {r.open ? "可领取" : "未开放"}
        </Badge>
      }
    >
      <div class="stack-sm">
        {!r.configured && <Notice>Discord 机器人尚未配置：以下设置会保存，机器人启用后才生效。</Notice>}
        <SwitchRow
          title="开放自助领取"
          desc={r.open ? "成员可通过 Discord /register 自助领取 Key" : "新成员暂时无法领取 Key"}
          checked={r.open}
          onChange={(v) => setToggleTo(v)}
        />
        <Meter
          label="名额"
          value={r.active}
          max={r.max_users}
          text={`${r.active} / ${r.max_users > 0 ? r.max_users : "不限"}`}
          {...(r.max_users > 0 && r.active >= r.max_users
            ? { tone: "danger" as const }
            : pct >= 80
              ? { tone: "warn" as const }
              : {})}
          hint="已领取并绑定 Discord 的成员数 / 名额上限"
        />

        <form class="stack-sm" {...form.formProps} onSubmit={(e) => e.preventDefault()}>
          <div class="form-grid">
            <NumField form={form} name="max_users" />
            <NumField form={form} name="expires_days" />
          </div>
          <fieldset class="checks">
            <legend class="form-section__title">限定身份组</legend>
            <p class="muted small">只有在指定服务器里拥有指定身份组的人能领 Key；留空 = 不限定。</p>
            <div class="form-grid">
              <TextField form={form} name="role_guild" inputMode="numeric" />
              <TextField form={form} name="role_id" inputMode="numeric" />
              <TextField form={form} name="role_note" />
            </div>
            {roleErr && (form.isDirty("role_guild") || form.isDirty("role_id")) && (
              <span class="field__error" role="alert">
                {roleErr}
              </span>
            )}
            <ExtraRoles server={r.role_extra ?? []} />
          </fieldset>

          <fieldset class="checks">
            <legend class="form-section__title">新成员默认开通的功能</legend>
            <div class="form-grid">
              <SelectField form={form} name="features_mode" label="范围" />
            </div>
            {customFeatures && (
              <>
                <div class="actions">
                  <Button
                    size="sm"
                    onClick={() => {
                      for (const n of names) form.set(FEATURE_PREFIX + n, true);
                    }}
                  >
                    全选
                  </Button>
                  <Button
                    size="sm"
                    onClick={() => {
                      for (const n of names) form.set(FEATURE_PREFIX + n, false);
                    }}
                  >
                    清空
                  </Button>
                </div>
                <div class="chips">
                  {names.map((n) => (
                    <label class="check" key={n}>
                      <input
                        type="checkbox"
                        checked={form.raw(FEATURE_PREFIX + n) === true}
                        onChange={(e) => form.set(FEATURE_PREFIX + n, (e.target as HTMLInputElement).checked)}
                      />
                      <span>{labels[n]}</span>
                    </label>
                  ))}
                </div>
              </>
            )}
          </fieldset>
          <SaveBar form={form} onSave={save} saving={saving} />
        </form>
      </div>

      <ConfirmDialog
        open={toggleTo !== null}
        title={toggleTo ? "开放自助领 Key？" : "暂停自助领 Key？"}
        confirmText={toggleTo ? "开放" : "暂停"}
        onClose={() => setToggleTo(null)}
        onConfirm={() => setOpen(toggleTo === true)}
      >
        <p>
          {toggleTo
            ? "开放后成员可以用 /register 自助领取。"
            : "暂停后新成员无法领取，已领取的成员不受影响。"}
        </p>
      </ConfirmDialog>
    </Card>
  );
}
