/**
 * 设置类分区（上游 / 领 Key 与功能 / 额度与保护 / Discord / 系统）共用的表单件。
 *
 * - useForm：按字段规格校验（规则同后端），只提交改过的字段；编辑期间暂停全局自动刷新（busy）。
 * - 开关、数字 / 文字 / 下拉字段、保存条、「推算过程」框。
 * 只用 ui 组件库与 tokens.css 的变量。
 */
import type { ComponentChildren } from "preact";
import { useId, useMemo, useState } from "preact/hooks";
import { Button } from "../../../../ui";
import { useBusy } from "../_kit";
import { type FieldSpec, parseField, type Raw, sameRaw, toRaw } from "./core";
import "./forms.css";

/* ---------------- 表单状态 ---------------- */
export type Form = {
  specs: Record<string, FieldSpec>;
  raw: (name: string) => Raw;
  set: (name: string, value: Raw) => void;
  error: (name: string) => string | null;
  isDirty: (name: string) => boolean;
  dirty: boolean;
  dirtyCount: number;
  valid: boolean;
  /** 已改且合法的字段 → 解析后的值（只提交这些） */
  changes: () => Record<string, number | string | boolean>;
  /** 保存成功后调用：在下一次从服务端取到新数据之前，先把改动当作已保存的值显示 */
  commit: () => void;
  reset: () => void;
  /** 绑在 <form> 上：聚焦期间暂停自动刷新 */
  formProps: {
    onFocusIn: () => void;
    onFocusOut: (e: FocusEvent) => void;
  };
};

/**
 * server：服务端当前值（字段名 → 值），null 表示还没加载。
 * 草稿只保存用户改过的字段，所以自动刷新带来的新数据不会冲掉正在输入的内容。
 */
export function useForm(specs: Record<string, FieldSpec>, server: Record<string, unknown> | null): Form {
  const [draft, setDraft] = useState<Record<string, Raw>>({});
  const [overlay, setOverlay] = useState<{ base: Record<string, unknown> | null; vals: Record<string, Raw> }>(
    {
      base: null,
      vals: {},
    },
  );
  const [focused, setFocused] = useState(false);

  const baseline = useMemo(() => {
    const out: Record<string, Raw> = {};
    for (const [name, spec] of Object.entries(specs)) out[name] = toRaw(spec, server?.[name]);
    // 刚保存完、新数据还没回来：先按已保存的值显示，避免输入框闪回旧值
    if (server && overlay.base === server) Object.assign(out, overlay.vals);
    return out;
  }, [specs, server, overlay]);

  const raw = (name: string): Raw => (name in draft ? (draft[name] as Raw) : (baseline[name] ?? ""));
  const isDirty = (name: string): boolean => {
    const spec = specs[name];
    return !!spec && name in draft && !sameRaw(spec, draft[name] as Raw, baseline[name] ?? "");
  };
  const names = Object.keys(specs);
  const dirtyNames = names.filter(isDirty);
  const errors: Record<string, string | null> = {};
  for (const name of dirtyNames) {
    const spec = specs[name] as FieldSpec;
    const p = parseField(spec, raw(name));
    errors[name] = p.ok ? null : p.error;
  }
  const dirty = dirtyNames.length > 0;
  useBusy(focused || dirty);

  return {
    specs,
    raw,
    set: (name, value) => {
      setDraft((d) => ({ ...d, [name]: value }));
    },
    error: (name) => errors[name] ?? null,
    isDirty,
    dirty,
    dirtyCount: dirtyNames.length,
    valid: dirtyNames.every((n) => !errors[n]),
    changes: () => {
      const out: Record<string, number | string | boolean> = {};
      for (const n of dirtyNames) {
        const p = parseField(specs[n] as FieldSpec, raw(n));
        if (p.ok) out[n] = p.value;
      }
      return out;
    },
    commit: () => {
      const vals: Record<string, Raw> = {};
      for (const n of dirtyNames) vals[n] = raw(n);
      setOverlay({ base: server, vals: { ...(overlay.base === server ? overlay.vals : {}), ...vals } });
      setDraft({});
    },
    reset: () => setDraft({}),
    formProps: {
      onFocusIn: () => setFocused(true),
      onFocusOut: (e) => {
        if (!(e.currentTarget as HTMLElement).contains(e.relatedTarget as Node | null)) setFocused(false);
      },
    },
  };
}

/* ---------------- 开关 ---------------- */
export function Switch(props: {
  checked: boolean;
  onChange: (v: boolean) => void;
  labelledBy?: string | undefined;
  describedBy?: string | undefined;
}) {
  return (
    <button
      type="button"
      role="switch"
      class="switch"
      aria-checked={props.checked}
      aria-labelledby={props.labelledBy}
      aria-describedby={props.describedBy}
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
        labelledBy={`${id}-t`}
        describedBy={props.desc ? `${id}-d` : undefined}
      />
    </div>
  );
}

/* ---------------- 字段 ---------------- */
function FieldShell(props: {
  id: string;
  label: ComponentChildren;
  hint?: ComponentChildren;
  error: string | null;
  children: ComponentChildren;
}) {
  return (
    <div class="field">
      <label class="field__label" for={props.id}>
        {props.label}
      </label>
      {props.children}
      {props.hint && (
        <span class="field__hint" id={`${props.id}-h`}>
          {props.hint}
        </span>
      )}
      {props.error && (
        <span class="field__error" id={`${props.id}-e`} role="alert">
          {props.error}
        </span>
      )}
    </div>
  );
}

const describedBy = (id: string, hint: unknown, error: string | null): string | undefined => {
  const parts = [hint ? `${id}-h` : "", error ? `${id}-e` : ""].filter(Boolean);
  return parts.length ? parts.join(" ") : undefined;
};

export function NumField(props: {
  form: Form;
  name: string;
  hint?: ComponentChildren;
  disabled?: boolean;
  /** 覆盖规格里的 label */
  label?: ComponentChildren;
}) {
  const id = useId();
  const spec = props.form.specs[props.name];
  if (spec?.kind !== "int" && spec?.kind !== "float") return null;
  const error = props.form.error(props.name);
  const hint = props.hint ?? spec.hint;
  return (
    <FieldShell id={id} label={props.label ?? spec.label} hint={hint} error={error}>
      <div class="inputwrap">
        <input
          id={id}
          class="input"
          type="text"
          inputMode={spec.kind === "int" ? "numeric" : "decimal"}
          autoComplete="off"
          value={String(props.form.raw(props.name))}
          disabled={props.disabled}
          aria-invalid={error ? "true" : undefined}
          aria-describedby={describedBy(id, hint, error)}
          onInput={(e) => props.form.set(props.name, (e.target as HTMLInputElement).value)}
        />
        {spec.unit && (
          <span class="inputwrap__unit" aria-hidden="true">
            {spec.unit}
          </span>
        )}
      </div>
    </FieldShell>
  );
}

export function TextField(props: { form: Form; name: string; inputMode?: "numeric" | "text" }) {
  const id = useId();
  const spec = props.form.specs[props.name];
  if (spec?.kind !== "text") return null;
  const error = props.form.error(props.name);
  const hint = spec.hint;
  return (
    <FieldShell id={id} label={spec.label} hint={hint} error={error}>
      <input
        id={id}
        class="input"
        type="text"
        inputMode={props.inputMode ?? "text"}
        autoComplete="off"
        value={String(props.form.raw(props.name))}
        aria-invalid={error ? "true" : undefined}
        aria-describedby={describedBy(id, hint, error)}
        onInput={(e) => props.form.set(props.name, (e.target as HTMLInputElement).value)}
      />
    </FieldShell>
  );
}

export function SelectField(props: {
  form: Form;
  name: string;
  hint?: ComponentChildren;
  disabled?: boolean;
  label?: ComponentChildren;
}) {
  const id = useId();
  const spec = props.form.specs[props.name];
  if (spec?.kind !== "select") return null;
  const error = props.form.error(props.name);
  return (
    <FieldShell id={id} label={props.label ?? spec.label} hint={props.hint} error={error}>
      <select
        id={id}
        class="select"
        value={String(props.form.raw(props.name))}
        disabled={props.disabled}
        aria-describedby={describedBy(id, props.hint, error)}
        onChange={(e) => props.form.set(props.name, (e.target as HTMLSelectElement).value)}
      >
        {spec.options.map((o) => (
          <option key={o.value} value={o.value}>
            {o.label}
          </option>
        ))}
      </select>
    </FieldShell>
  );
}

/* ---------------- 保存条 ---------------- */
/** 卡片底部：一个「保存」，只在有改动且合法时可点；有改动时可「撤销修改」。 */
export function SaveBar(props: { form: Form; onSave: () => unknown; saving?: boolean; label?: string }) {
  const f = props.form;
  return (
    <div class="savebar">
      <Button
        type="submit"
        variant="primary"
        disabled={!f.dirty || !f.valid || props.saving}
        onClick={(e) => {
          e.preventDefault();
          void props.onSave();
        }}
      >
        {props.saving ? "保存中…" : (props.label ?? "保存")}
      </Button>
      {f.dirty && (
        <Button variant="ghost" onClick={f.reset} disabled={props.saving}>
          撤销修改
        </Button>
      )}
      <span class="savebar__state" role="status">
        {f.dirty ? (f.valid ? `有 ${f.dirtyCount} 项未保存` : "有输入不合法，请先修改") : "暂无修改"}
      </span>
    </div>
  );
}

/* ---------------- 展示件 ---------------- */
/** 「推算过程」框：把决定数字的来源摆出来，方便站长用常识检查。 */
export function Derive(props: { title?: string; children: ComponentChildren }) {
  return (
    <div class="derive">
      <div class="derive__title">{props.title ?? "推算过程"}</div>
      <div class="derive__body">{props.children}</div>
    </div>
  );
}

/** 名称 / 值 列表 */
export function Kv(props: { rows: Array<[ComponentChildren, ComponentChildren]> }) {
  return (
    <dl class="kv">
      {props.rows.map(([k, v], i) => (
        <div class="kv__row" key={i}>
          <dt>{k}</dt>
          <dd>{v}</dd>
        </div>
      ))}
    </dl>
  );
}

/** 给 ui 组件传可选的 tone：exactOptionalPropertyTypes 下不能直接传 undefined */
export const withTone = <T extends string>(tone: T | undefined | false): { tone?: T } =>
  tone ? { tone } : {};

/** 分区根容器：保证表格等宽内容不会把页面撑出横向滚动条 */
export function Sect(props: { children: ComponentChildren }) {
  return <div class="sect">{props.children}</div>;
}
