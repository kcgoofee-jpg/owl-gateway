/**
 * 工作台本地小组件。FieldBlock：与 kit 的 Field 同款观感，但外层是 <div> 不是 <label>——
 * label 只能关联一个控件，包住按钮组 / 多控件组合时点击语义会乱（happy-dom 还会把按钮点击
 * 转发给第一个可标注控件）。单控件的字段仍用 kit 的 Field。
 */
import type { ComponentChildren } from "preact";

export function FieldBlock(props: {
  label: ComponentChildren;
  hint?: ComponentChildren;
  children: ComponentChildren;
}) {
  return (
    <div class="field wb-fieldblock">
      <span class="field__label">{props.label}</span>
      {props.children}
      {props.hint && <span class="field__hint">{props.hint}</span>}
    </div>
  );
}
