import type { ComponentChildren } from "preact";
import { useId } from "preact/hooks";
/* ---------------- 开关 ---------------- */
export function Switch(props: {
  checked: boolean;
  onChange: (v: boolean) => void;
  labelledBy?: string | undefined;
  describedBy?: string | undefined;
  disabled?: boolean | undefined;
}) {
  return (
    <button
      type="button"
      role="switch"
      class="switch"
      aria-checked={props.checked}
      aria-labelledby={props.labelledBy}
      aria-describedby={props.describedBy}
      disabled={props.disabled}
      onClick={() => props.onChange(!props.checked)}
    >
      <span class="switch__thumb" aria-hidden="true" />
    </button>
  );
}

/** 一行：左边标题 + 说明，右边开关 */
export function SwitchRow(props: {
  title: ComponentChildren;
  desc?: ComponentChildren;
  checked: boolean;
  onChange: (v: boolean) => void;
  disabled?: boolean | undefined;
  /** 标题右侧的小标记 */
  tag?: ComponentChildren;
}) {
  const id = useId();
  return (
    <div class="switch-row">
      <div class="switch-row__body">
        <div class="switch-row__title" id={`${id}-t`}>
          {props.title}
          {props.tag}
        </div>
        {props.desc && (
          <div class="switch-row__desc" id={`${id}-d`}>
            {props.desc}
          </div>
        )}
      </div>
      <Switch
        checked={props.checked}
        onChange={props.onChange}
        disabled={props.disabled}
        labelledBy={`${id}-t`}
        describedBy={props.desc ? `${id}-d` : undefined}
      />
    </div>
  );
}
