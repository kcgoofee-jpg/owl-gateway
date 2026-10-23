/** 全站功能开关。 */
import { useMemo, useState } from "preact/hooks";
import { Badge, Card, Notice, toast } from "../../../../ui";
import { actOrError, ConfirmDialog } from "../_kit";
import type { FieldSpec } from "../system/core";
import { SaveBar, SwitchRow, useForm } from "../system/forms";
import { ANLAS_FEATURES, type Ops } from "./core";

export function FeaturesCard(props: { ops: Ops }) {
  const { global, labels } = props.ops.features;
  const names = Object.keys(labels);
  const specs = useMemo<Record<string, FieldSpec>>(() => {
    const o: Record<string, FieldSpec> = {};
    for (const n of names) o[n] = { kind: "bool", label: labels[n] ?? n };
    return o;
  }, [labels, names.join(",")]);
  const form = useForm(specs, global);
  const [saving, setSaving] = useState(false);
  const [confirm, setConfirm] = useState(false);

  const ch = form.changes();
  const turningOn = Object.keys(ch).filter((n) => ch[n] === true && ANLAS_FEATURES[n]);
  const turningOff = Object.keys(ch).filter((n) => ch[n] === false);

  /** 返回 undefined = 已保存；返回字符串 = 失败原因 */
  const save = async () => {
    setSaving(true);
    const err = await actOrError("PUT", "/admin/api/ops/features", { flags: ch }, "功能开关已保存");
    setSaving(false);
    if (!err) form.commit();
    return err;
  };

  return (
    <Card title="功能开关" sub="关闭后所有成员立即不可用；管理员 Key 仍可使用。">
      <form {...form.formProps} onSubmit={(e) => e.preventDefault()}>
        {names.map((n) => (
          <SwitchRow
            key={n}
            title={labels[n]}
            {...(ANLAS_FEATURES[n]
              ? {
                  tag: (
                    <Badge tone="warn" title="此功能消耗上游官方付费点数（Anlas）">
                      消耗 Anlas
                    </Badge>
                  ),
                  desc: ANLAS_FEATURES[n],
                }
              : {})}
            checked={form.raw(n) === true}
            onChange={(v) => form.set(n, v)}
          />
        ))}
        <SaveBar
          form={form}
          saving={saving}
          onSave={() =>
            turningOn.length || turningOff.length
              ? setConfirm(true)
              : void save().then((err) => err && toast(err, { error: true }))
          }
        />
      </form>
      <ConfirmDialog
        open={confirm}
        title="确认修改功能开关？"
        confirmText="确认保存"
        danger={turningOff.length > 0}
        onClose={() => setConfirm(false)}
        onConfirm={save}
      >
        {turningOff.length > 0 && (
          <Notice tone="warn">
            关闭「{turningOff.map((n) => labels[n]).join("」「")}」后，所有成员（管理员 Key
            除外）立即不能使用。
          </Notice>
        )}
        {turningOn.length > 0 && (
          <Notice tone="warn">
            开启「{turningOn.map((n) => labels[n]).join("」「")}」会允许成员使用消耗官方
            Anlas（付费点数）的功能，可能消耗上游账号的付费点数余额。
          </Notice>
        )}
      </ConfirmDialog>
    </Card>
  );
}
