import { Button } from "../../../ui";
import type { FieldSpec } from "./system/core";
import { parseField, toRaw } from "./system/core";
import { type Form, NumField, SwitchRow } from "./system/forms";
import "./member-rules.css";

import { FEATURE_FIELDS, type MemberPolicy, NUMBER_FIELDS } from "../../../lib/member-policy";

export * from "../../../lib/member-policy";
export const POLICY_SPECS: Record<keyof MemberPolicy, FieldSpec> = {
  daily_images: { kind: "int", label: "V4.5 免费图/天", min: 0, max: 9999, unit: "张" },
  daily_v5: { kind: "int", label: "V5 每天", min: 0, max: 9999, unit: "张" },
  daily_anlas: { kind: "int", label: "Anlas 每天", min: 0, max: 9999, unit: "Anlas" },
  monthly_anlas: { kind: "float", label: "Anlas 每月", min: 0, max: 99999, unit: "Anlas" },
  rpm: { kind: "int", label: "每分钟请求数", min: 1, max: 600, unit: "次" },
  image: { kind: "bool", label: "图片生成" },
  v5: { kind: "bool", label: "V5" },
  vibe: { kind: "bool", label: "Vibe 与参考图" },
  tags: { kind: "bool", label: "标签补全" },
  img2img: { kind: "bool", label: "图生图与局部重绘" },
  upscale: { kind: "bool", label: "放大" },
  augment: { kind: "bool", label: "导演工具" },
  paid_parameters: { kind: "bool", label: "高规格生图" },
};
export function policyFromForm(base: MemberPolicy, form: Form): MemberPolicy {
  return { ...base, ...form.changes() } as MemberPolicy;
}
export function PolicyFields({ form, defaults }: { form: Form; defaults?: MemberPolicy }) {
  const source = (name: keyof MemberPolicy) => {
    if (!defaults) return null;
    const parsed = parseField(POLICY_SPECS[name], form.raw(name));
    const exception = !parsed.ok || parsed.value !== defaults[name];
    return (
      <div class="mr-source">
        <span>{exception ? "例外" : "默认"}</span>
        {exception && (
          <Button
            size="sm"
            variant="ghost"
            aria-label={`恢复${POLICY_SPECS[name].label}默认`}
            onClick={() => form.set(name, toRaw(POLICY_SPECS[name], defaults[name]))}
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
            <SwitchRow
              title={POLICY_SPECS[name].label}
              checked={form.raw(name) === true}
              onChange={(value) => form.set(name, value)}
            />
            {source(name)}
          </div>
        ))}
      </div>
    </>
  );
}
