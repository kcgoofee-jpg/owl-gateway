/**
 * 领 Key 门槛：主身份组一行 + 额外的限定身份组列表，合并成一张卡、一个「保存」。
 * 和服务端一样一次提交：PUT /ops/registration 的请求体里，改了哪个部分就带哪个部分。
 */
import { Plus, X } from "lucide-preact";
import { useEffect, useMemo, useRef, useState } from "preact/hooks";
import { Button, Card, toast } from "../../../../ui";
import { actOrError } from "../_kit";
import { SaveBar, TextField, useForm } from "../system/forms";
import {
  extraChanged,
  extraRoleProblem,
  MAX_ROLE_EXTRA,
  normalizeExtra,
  type Ops,
  ROLE_GATE_SPECS,
  type RoleRule,
  regBody,
  roleProblem,
} from "./core";

const URL = "/admin/api/ops/registration";
const blank = (): RoleRule => ({ guild: "", role: "", note: "" });

export default function RoleGateCard(props: { ops: Ops }) {
  const r = props.ops.registration;
  const serverExtra = r.role_extra ?? [];
  const serverKey = JSON.stringify(serverExtra);

  const roleServer = useMemo(
    () => ({ role_guild: r.role_guild, role_id: r.role_id, role_note: r.role_note }),
    [r],
  );
  const form = useForm(ROLE_GATE_SPECS, roleServer);
  const [rows, setRows] = useState<RoleRule[]>(serverExtra);
  const [saving, setSaving] = useState(false);
  const prevExtra = useRef<RoleRule[]>(serverExtra);
  // 服务端的值变了（保存后 / 别处改了）且本地没有未保存的改动：跟着更新
  useEffect(() => {
    const next = JSON.parse(serverKey) as RoleRule[];
    const prev = prevExtra.current;
    prevExtra.current = next;
    setRows((cur) => (extraChanged(cur, prev) ? cur : next));
  }, [serverKey]);

  const roleErr = roleProblem(String(form.raw("role_guild")), String(form.raw("role_id")));
  const extraProblem = extraRoleProblem(rows);
  const extraDirty = extraChanged(rows, serverExtra);
  const set = (i: number, patch: Partial<RoleRule>) =>
    setRows((cur) => cur.map((row, j) => (j === i ? { ...row, ...patch } : row)));
  /** 保存条看到的表单：把额外身份组的改动算进来，一张卡一个「保存」 */
  const view = {
    ...form,
    dirty: form.dirty || extraDirty,
    dirtyCount: form.dirtyCount + (extraDirty ? 1 : 0),
    valid: form.valid && !extraProblem,
    reset: () => {
      form.reset();
      setRows(serverExtra);
    },
  };

  const save = async () => {
    if (roleErr || extraProblem) {
      toast(roleErr ?? extraProblem ?? "", { error: true });
      return;
    }
    const body = regBody(form.changes(), form.raw as (n: string) => string | boolean, []);
    if (extraDirty) body.role_extra = normalizeExtra(rows);
    setSaving(true);
    const err = await actOrError("PUT", URL, body, "领 Key 门槛已保存");
    setSaving(false);
    if (err) toast(err, { error: true });
    else form.commit();
  };

  return (
    <Card
      title="领 Key 门槛"
      sub="只有在指定服务器里拥有指定身份组的人能领 Key；额外身份组满足任意一个即可，可来自不同服务器。"
    >
      <form class="stack-sm" {...form.formProps} onSubmit={(e) => e.preventDefault()}>
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

        {rows.map((row, i) => (
          <div class="form-grid" key={i}>
            <input
              class="input mono"
              inputMode="numeric"
              placeholder="服务器 ID"
              aria-label={`额外身份组 ${i + 1} 的服务器 ID`}
              value={row.guild}
              onInput={(e) => set(i, { guild: e.currentTarget.value })}
            />
            <input
              class="input mono"
              inputMode="numeric"
              placeholder="身份组 ID"
              aria-label={`额外身份组 ${i + 1} 的身份组 ID`}
              value={row.role}
              onInput={(e) => set(i, { role: e.currentTarget.value })}
            />
            <div class="joined">
              <input
                class="input"
                maxLength={60}
                placeholder="显示名，例如 棱镜「创作者」"
                aria-label={`额外身份组 ${i + 1} 的显示名`}
                value={row.note}
                onInput={(e) => set(i, { note: e.currentTarget.value })}
              />
              <Button
                variant="ghost"
                icon
                aria-label={`删除额外身份组 ${i + 1}`}
                onClick={() => setRows((cur) => cur.filter((_, j) => j !== i))}
              >
                <X size={16} aria-hidden="true" />
              </Button>
            </div>
          </div>
        ))}
        {extraProblem && extraDirty && (
          <span class="field__error" role="alert">
            {extraProblem}
          </span>
        )}

        <div class="row">
          <Button
            disabled={rows.length >= MAX_ROLE_EXTRA}
            onClick={() => setRows((cur) => [...cur, blank()])}
          >
            <Plus size={15} aria-hidden="true" />
            添加一个身份组
          </Button>
        </div>
        <SaveBar form={view} onSave={save} saving={saving} />
      </form>
    </Card>
  );
}
