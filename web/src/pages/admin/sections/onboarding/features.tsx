import { useMemo, useState } from "preact/hooks";
import { Card } from "../../../../ui";
import { act } from "../../Admin";
import type { FieldSpec } from "../system/core";
import { SaveBar, SwitchRow, useForm } from "../system/forms";
import type { Ops } from "./core";
export function FeaturesCard({ ops }: { ops: Ops }) {
  const { global, labels } = ops.features;
  const specs = useMemo(
    () =>
      Object.fromEntries(
        Object.entries(labels).map(([name, label]) => [name, { kind: "bool", label } satisfies FieldSpec]),
      ),
    [labels],
  );
  const form = useForm(specs, global);
  const [saving, setSaving] = useState(false);
  const save = async () => {
    setSaving(true);
    const ok = await act("PUT", "/admin/api/ops/features", { flags: form.changes() }, "全站功能已保存");
    setSaving(false);
    if (ok) form.commit();
  };
  return (
    <Card title="全站功能">
      <form {...form.formProps} onSubmit={(event) => event.preventDefault()}>
        {Object.entries(labels).map(([name, label]) => (
          <SwitchRow
            key={name}
            title={label}
            checked={form.raw(name) === true}
            onChange={(value) => form.set(name, value)}
          />
        ))}
        <SaveBar form={form} saving={saving} onSave={save} />
      </form>
    </Card>
  );
}
