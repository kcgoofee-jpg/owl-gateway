import { Button } from "../../../ui";
import type { FieldSpec } from "./system/core";
import { parseField, toRaw } from "./system/core";
import { type Form, NumField, SwitchRow } from "./system/forms";
import "./member-rules.css";

import {
  FEATURE_COPY,
  FEATURE_FIELDS,
  type MemberOverrides,
  type MemberPolicy,
  NUMBER_FIELDS,
  POLICY_FIELDS,
} from "../../../lib/member-policy";

export * from "../../../lib/member-policy";

/** 接口 `fields` 里的一项。数字范围以服务端为准。 */
export type PolicyBound = { kind: "int" | "float" | "bool"; min?: number; max?: number };

const POLICY_LABELS: Record<keyof MemberPolicy, { label: string; unit?: string }> = {
  daily_images: { label: "V4.5 免费图/天", unit: "张" },
  daily_v5: { label: "V5 每天", unit: "张" },
  daily_anlas: { label: "Anlas 每天", unit: "Anlas" },
  monthly_anlas: { label: "Anlas 每月", unit: "Anlas" },
  rpm: { label: "每分钟请求数", unit: "次" },
  image: FEATURE_COPY.image,
  v5: FEATURE_COPY.v5,
  vibe: FEATURE_COPY.vibe,
  tags: FEATURE_COPY.tags,
  img2img: FEATURE_COPY.img2img,
  upscale: FEATURE_COPY.upscale,
  augment: FEATURE_COPY.augment,
  paid_parameters: FEATURE_COPY.paid_parameters,
  share_exempt: FEATURE_COPY.share_exempt,
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
  return { ...base, ...form.values() } as MemberPolicy;
}
export function PolicyFields({
  form,
  defaults,
  fullImage = false,
  overrides,
  resetFields,
  onReset,
  onEdit,
}: {
  form: Form;
  defaults?: MemberPolicy;
  fullImage?: boolean;
  overrides?: MemberOverrides;
  resetFields?: ReadonlySet<keyof MemberPolicy>;
  onReset?: (name: keyof MemberPolicy) => void;
  onEdit?: (name: keyof MemberPolicy) => void;
}) {
  const source = (name: keyof MemberPolicy) => {
    if (!defaults) return null;
    const spec = form.specs[name];
    if (!spec) return null;
    const parsed = parseField(spec, form.raw(name));
    const exception = form.isDirty(name)
      ? !parsed.ok || parsed.value !== defaults[name]
      : !resetFields?.has(name) && overrides?.[name] != null;
    return (
      <div class="mr-source">
        <span>{exception ? "例外" : "默认"}</span>
        {exception && (
          <Button
            size="sm"
            variant="ghost"
            aria-label={`恢复${spec.label}默认`}
            onClick={() => {
              form.set(name, toRaw(spec, defaults[name]));
              onReset?.(name);
            }}
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
          <div key={name} onInput={() => onEdit?.(name)}>
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
                onChange={(value) => {
                  onEdit?.(name);
                  form.set(name, value);
                }}
              />
            )}
            {!fullImage && source(name)}
          </div>
        ))}
      </div>
    </>
  );
}
