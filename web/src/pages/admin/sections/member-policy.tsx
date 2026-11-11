import { Button } from "../../../ui";
import type { FieldSpec } from "./system/core";
import { parseField, toRaw } from "./system/core";
import { type Form, NumField, SwitchRow } from "./system/forms";
import "./member-rules.css";

import { FEATURE_FIELDS, type MemberPolicy, NUMBER_FIELDS, POLICY_FIELDS } from "../../../lib/member-policy";

export * from "../../../lib/member-policy";

/** 接口 `fields` 里的一项。数字范围以服务端为准。 */
export type PolicyBound = { kind: "int" | "float" | "bool"; min?: number; max?: number };

const POLICY_LABELS: Record<keyof MemberPolicy, { label: string; unit?: string }> = {
  daily_images: { label: "V4.5 免费图/天", unit: "张" },
  daily_v5: { label: "V5 每天", unit: "张" },
  daily_anlas: { label: "Anlas 每天", unit: "Anlas" },
  monthly_anlas: { label: "Anlas 每月", unit: "Anlas" },
  rpm: { label: "每分钟请求数", unit: "次" },
  image: { label: "图片生成" },
  v5: { label: "V5" },
  vibe: { label: "Vibe 与参考图" },
  tags: { label: "标签补全" },
  img2img: { label: "图生图与局部重绘" },
  upscale: { label: "放大" },
  augment: { label: "导演工具" },
  paid_parameters: { label: "高规格生图（大图、多步数与批量）" },
};

export const policyLabel = (name: keyof MemberPolicy): string => POLICY_LABELS[name].label;

/** 中文标签留在前端，上下限用接口给的 fields。 */
export function policySpecs(fields: Record<string, PolicyBound>): Record<keyof MemberPolicy, FieldSpec> {
  const out = {} as Record<keyof MemberPolicy, FieldSpec>;
  for (const name of POLICY_FIELDS) {
    const label = POLICY_LABELS[name].label;
    const bound = fields[name];
    if (!bound || bound.kind === "bool") {
      out[name] = { kind: "bool", label };
      continue;
    }
    out[name] = {
      kind: bound.kind,
      label,
      min: bound.min ?? 0,
      max: bound.max ?? 0,
      ...(POLICY_LABELS[name].unit ? { unit: POLICY_LABELS[name].unit } : {}),
    };
  }
  return out;
}
export function policyFromForm(base: MemberPolicy, form: Form): MemberPolicy {
  return { ...base, ...form.changes() } as MemberPolicy;
}
export function PolicyFields({
  form,
  defaults,
  fullImage = false,
}: {
  form: Form;
  defaults?: MemberPolicy;
  fullImage?: boolean;
}) {
  const source = (name: keyof MemberPolicy) => {
    if (!defaults) return null;
    const spec = form.specs[name];
    if (!spec) return null;
    const parsed = parseField(spec, form.raw(name));
    const exception = !parsed.ok || parsed.value !== defaults[name];
    return (
      <div class="mr-source">
        <span>{exception ? "例外" : "默认"}</span>
        {exception && (
          <Button
            size="sm"
            variant="ghost"
            aria-label={`恢复${spec.label}默认`}
            onClick={() => form.set(name, toRaw(spec, defaults[name]))}
          >
            恢复默认
          </Button>
        )}
      </div>
    );
  };
  return (
    <>
      <div class="form-section__title">额度 · 0 停用</div>
      <div class="form-grid mr-numbers">
        {NUMBER_FIELDS.map((name) => (
          <div key={name}>
            <NumField form={form} name={name} />
            {source(name)}
          </div>
        ))}
      </div>
      <div class="form-section mr-features">
        {FEATURE_FIELDS.map((name) => (
          <div key={name} class="mr-feature">
            {fullImage ? (
              <div>{policyLabel(name)} · 已开启</div>
            ) : (
              <SwitchRow
                title={policyLabel(name)}
                checked={form.raw(name) === true}
                onChange={(value) => form.set(name, value)}
              />
            )}
            {!fullImage && source(name)}
          </div>
        ))}
      </div>
    </>
  );
}
