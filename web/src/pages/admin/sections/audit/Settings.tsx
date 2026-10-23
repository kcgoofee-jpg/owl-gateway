/** 生成记录：是否记录提示词 / 保存原图、保留时长与容量上限。 */
import { useMemo, useState } from "preact/hooks";
import { Card, Meter } from "../../../../ui";
import { act } from "../../Admin";
import { AUDIT_SPECS, auditServer, type Ops } from "../onboarding/core";
import { bytesText, mbText } from "../system/core";
import { NumField, SaveBar, SwitchRow, useForm } from "../system/forms";

const URL = "/admin/api/ops/audit";

export default function AuditCard(props: { ops: Ops }) {
  const a = props.ops.audit;
  const server = useMemo(() => auditServer(a), [a]);
  const form = useForm(AUDIT_SPECS, server);
  const [saving, setSaving] = useState(false);
  const save = async () => {
    setSaving(true);
    const ok = await act("PUT", URL, form.changes(), "生成记录设置已保存");
    setSaving(false);
    if (ok) form.commit();
  };
  const usedMb = a.image_bytes / 1048576;
  return (
    <Card title="记录">
      <form {...form.formProps} onSubmit={(e) => e.preventDefault()}>
        <SwitchRow
          title="记录提示词"
          checked={form.raw("prompts") === true}
          onChange={(v) => form.set("prompts", v)}
        />
        <SwitchRow
          title="保存原图"
          desc="到保存时长就删，另外超过容量上限时删最旧的"
          checked={form.raw("thumbs") === true}
          onChange={(v) => form.set("thumbs", v)}
        />
        <div class="form-grid" style={{ marginTop: "var(--sp-4)" }}>
          <NumField form={form} name="image_keep_hours" />
          <NumField form={form} name="retention_days" />
          <NumField form={form} name="origin_keep_days" />
          <NumField form={form} name="image_cap_mb" />
        </div>
        {a.image_cap_mb > 0 && (
          <div style={{ marginTop: "var(--sp-4)" }}>
            <Meter
              label="原图现在占用"
              value={usedMb}
              max={a.image_cap_mb}
              text={`${bytesText(a.image_bytes)} / ${mbText(a.image_cap_mb)}`}
              hint="存成文件、不进数据库"
            />
          </div>
        )}
        <SaveBar form={form} onSave={save} saving={saving} />
      </form>
    </Card>
  );
}
