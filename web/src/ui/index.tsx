/**
 * 猫头鹰公益站组件库。所有页面只用这里的组件和 tokens.css 的变量，不各自再写一套。
 * 规则：不出现钥匙图标（biome 拦截）；无障碍属性齐全（progressbar / dialog / aria-live / tablist）。
 */
import { signal } from "@preact/signals";
import { Check, Copy, X } from "lucide-preact";
import type { ButtonHTMLAttributes, ComponentChildren } from "preact";
import { useEffect, useId, useRef, useState } from "preact/hooks";
import "./tokens.css";
import "./base.css";
import "./components.css";

export { SearchMultiSelect } from "./SearchMultiSelect";
export { type SearchOption, SearchSelect } from "./SearchSelect";

type Cls = string | false | null | undefined;
export const cx = (...c: Cls[]): string => c.filter(Boolean).join(" ");

/* ---------------- Button ---------------- */
type ButtonProps = ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: "primary" | "ghost" | "danger" | "default";
  size?: "sm" | "md" | "lg";
  icon?: boolean;
};
export function Button({
  variant = "default",
  size = "md",
  icon,
  class: c,
  type = "button",
  ...rest
}: ButtonProps) {
  return (
    <button
      type={type}
      class={cx(
        "btn",
        variant !== "default" && `btn--${variant}`,
        size !== "md" && `btn--${size}`,
        icon && "btn--icon",
        c as string,
      )}
      {...rest}
    />
  );
}

/* ---------------- Card ---------------- */
export function Card(props: {
  title?: ComponentChildren;
  sub?: ComponentChildren;
  actions?: ComponentChildren;
  class?: string;
  id?: string;
  children?: ComponentChildren;
}) {
  const hid = useId();
  return (
    <section id={props.id} class={cx("card", props.class)} aria-labelledby={props.title ? hid : undefined}>
      {(props.title || props.actions) && (
        <header class="card__head">
          <div>
            {props.title && (
              <h2 class="card__title" id={hid}>
                {props.title}
              </h2>
            )}
            {props.sub && <p class="card__sub">{props.sub}</p>}
          </div>
          <span class="spacer" />
          {props.actions}
        </header>
      )}
      {props.children}
    </section>
  );
}

/* ---------------- Badge ---------------- */
export function Badge(props: {
  tone?: "ok" | "warn" | "danger" | "info" | "accent";
  dot?: boolean;
  children: ComponentChildren;
  title?: string;
}) {
  return (
    <span class={cx("badge", props.tone && `badge--${props.tone}`)} title={props.title}>
      {props.dot && <span class="dot" aria-hidden="true" />}
      {props.children}
    </span>
  );
}

/* ---------------- Meter ---------------- */
export function Meter(props: {
  label: ComponentChildren;
  value: number;
  max: number;
  /** 右侧显示的文字，默认「value / max」 */
  text?: ComponentChildren;
  hint?: ComponentChildren;
  tone?: "v5" | "warn" | "danger";
}) {
  const max = Math.max(0, props.max);
  const value = Math.max(0, Math.min(props.value, max || props.value));
  const pct = max > 0 ? Math.min(100, (value / max) * 100) : 0;
  const lid = useId();
  return (
    <div class={cx("meter", props.tone && `meter--${props.tone}`)}>
      <div class="meter__label" id={lid}>
        <span>{props.label}</span>
        <span class="meter__value">
          {props.text ?? (
            <>
              {props.value} / {props.max}
            </>
          )}
        </span>
      </div>
      <div
        class="meter__track"
        role="progressbar"
        aria-labelledby={lid}
        aria-valuemin={0}
        aria-valuemax={max}
        aria-valuenow={value}
      >
        <div class="meter__fill" style={{ width: `${pct}%` }} />
      </div>
      {props.hint && <p class="meter__hint">{props.hint}</p>}
    </div>
  );
}

/* ---------------- Dialog（原生 <dialog>：自带焦点陷阱与 Esc 关闭） ---------------- */
export function Dialog(props: {
  open: boolean;
  title: ComponentChildren;
  onClose: () => void;
  children: ComponentChildren;
  footer?: ComponentChildren;
  wide?: boolean;
}) {
  const ref = useRef<HTMLDialogElement>(null);
  const tid = useId();
  useEffect(() => {
    const d = ref.current;
    if (!d) return;
    if (props.open && !d.open) d.showModal();
    if (!props.open && d.open) d.close();
  }, [props.open]);
  return (
    // biome-ignore lint/a11y/useKeyWithClickEvents: 点遮罩关闭只是鼠标的便利；键盘用原生 <dialog> 的 Esc
    <dialog
      ref={ref}
      class="dialog"
      aria-labelledby={tid}
      onClose={props.onClose}
      onClick={(e) => e.target === ref.current && props.onClose()}
      style={props.wide ? { width: "min(880px, calc(100vw - 32px))" } : undefined}
    >
      <header class="dialog__head">
        <h2 id={tid} class="card__title">
          {props.title}
        </h2>
        <span class="spacer" />
        <Button variant="ghost" icon aria-label="关闭" onClick={props.onClose}>
          <X size={18} aria-hidden="true" />
        </Button>
      </header>
      <div class="dialog__body">{props.children}</div>
      {props.footer && <footer class="dialog__foot">{props.footer}</footer>}
    </dialog>
  );
}

/* ---------------- Toast ---------------- */
type ToastItem = { id: number; text: string; error: boolean };
const toastList = signal<ToastItem[]>([]);
let toastSeq = 0;
export function toast(text: string, opts: { error?: boolean; ms?: number } = {}): void {
  const id = ++toastSeq;
  toastList.value = [...toastList.value.slice(-3), { id, text, error: !!opts.error }];
  setTimeout(
    () => {
      toastList.value = toastList.value.filter((t) => t.id !== id);
    },
    opts.ms ?? (opts.error ? 6000 : 2600),
  );
}
export function Toaster() {
  return (
    <div class="toasts" role="status" aria-live="polite">
      {toastList.value.map((t) => (
        <div key={t.id} class={cx("toast", t.error && "toast--error")}>
          {t.text}
        </div>
      ))}
    </div>
  );
}

/* ---------------- Tabs（方向键切换） ---------------- */
export function Tabs(props: {
  tabs: Array<{ id: string; label: ComponentChildren; panel: () => ComponentChildren }>;
  label: string;
}) {
  const [active, setActive] = useState(props.tabs[0]?.id ?? "");
  const base = useId();
  const onKey = (e: KeyboardEvent) => {
    const i = props.tabs.findIndex((t) => t.id === active);
    const step = e.key === "ArrowRight" ? 1 : e.key === "ArrowLeft" ? -1 : 0;
    if (!step) return;
    e.preventDefault();
    const next = props.tabs[(i + step + props.tabs.length) % props.tabs.length];
    if (next) {
      setActive(next.id);
      document.getElementById(`${base}-tab-${next.id}`)?.focus();
    }
  };
  const current = props.tabs.find((t) => t.id === active);
  return (
    <div class="tabs">
      <div class="tabs__list" role="tablist" aria-label={props.label} onKeyDown={onKey}>
        {props.tabs.map((t) => (
          <button
            key={t.id}
            type="button"
            role="tab"
            id={`${base}-tab-${t.id}`}
            class="tabs__tab"
            aria-selected={t.id === active}
            aria-controls={`${base}-panel`}
            tabIndex={t.id === active ? 0 : -1}
            onClick={() => setActive(t.id)}
          >
            {t.label}
          </button>
        ))}
      </div>
      <div class="tabs__panel" role="tabpanel" id={`${base}-panel`} aria-labelledby={`${base}-tab-${active}`}>
        {current?.panel()}
      </div>
    </div>
  );
}

/* ---------------- CopyField ---------------- */
export async function copyText(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    // 非安全上下文 / 权限被拒：退回选中文本 + execCommand
    const ta = document.createElement("textarea");
    ta.value = text;
    ta.setAttribute("readonly", "");
    ta.className = "sr-only";
    document.body.append(ta);
    ta.select();
    const ok = document.execCommand("copy");
    ta.remove();
    return ok;
  }
}

export function CopyField(props: { value: string; label: string; display?: string; wrap?: boolean }) {
  const [done, setDone] = useState(false);
  return (
    <div class={props.wrap ? "copy copy--wrap" : "copy"}>
      <code class="copy__value" title={props.label}>
        {props.display ?? props.value}
      </code>
      <button
        type="button"
        class="copy__btn"
        aria-label={`复制${props.label}`}
        onClick={async () => {
          const ok = await copyText(props.value);
          if (ok) {
            setDone(true);
            setTimeout(() => setDone(false), 1500);
          } else toast("复制失败，请手动选择复制", { error: true });
        }}
      >
        {done ? <Check size={14} aria-hidden="true" /> : <Copy size={14} aria-hidden="true" />}
        <span class="copy__btn-text">{done ? "已复制" : "复制"}</span>
      </button>
    </div>
  );
}

/* ---------------- Menu（方向键 / Esc） ---------------- */
export function Menu(props: {
  button: (p: {
    "aria-expanded": boolean;
    "aria-haspopup": "menu";
    onClick: () => void;
  }) => ComponentChildren;
  items: Array<{ label: ComponentChildren; onSelect: () => void; danger?: boolean }>;
  /** 菜单顶部不可点的一行（如账户菜单里的「已登录：某某」） */
  header?: ComponentChildren;
}) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const first = ref.current?.querySelector<HTMLButtonElement>(".menu__item");
    first?.focus();
    const onDoc = (e: MouseEvent) => {
      if (!ref.current?.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener("click", onDoc);
    return () => document.removeEventListener("click", onDoc);
  }, [open]);
  const onKey = (e: KeyboardEvent) => {
    const items = [...(ref.current?.querySelectorAll<HTMLButtonElement>(".menu__item") ?? [])];
    const i = items.indexOf(document.activeElement as HTMLButtonElement);
    if (e.key === "Escape") setOpen(false);
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      const n = items.length;
      items[(i + (e.key === "ArrowDown" ? 1 : -1) + n) % n]?.focus();
    }
  };
  return (
    // biome-ignore lint/a11y/noStaticElementInteractions: 键盘事件委托给菜单容器（方向键在菜单项之间移动）
    <div class="menu" ref={ref} onKeyDown={onKey}>
      {props.button({ "aria-expanded": open, "aria-haspopup": "menu", onClick: () => setOpen(!open) })}
      {open && (
        <div class="menu__list" role="menu">
          {props.header && (
            <div class="menu__header" role="presentation">
              {props.header}
            </div>
          )}
          {props.items.map((it, i) => (
            <div key={i} role="none">
              <button
                type="button"
                role="menuitem"
                class={cx("menu__item", it.danger && "btn--danger")}
                onClick={() => {
                  setOpen(false);
                  it.onSelect();
                }}
              >
                {it.label}
              </button>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

/* ---------------- Skeleton ---------------- */
export function Skeleton(props: { w?: string; h?: string }) {
  return (
    <span class="skel" aria-hidden="true" style={{ width: props.w ?? "100%", height: props.h ?? "1em" }} />
  );
}

/* ---------------- Field ---------------- */
export function Field(props: {
  label: ComponentChildren;
  hint?: ComponentChildren;
  children: ComponentChildren;
}) {
  return (
    // biome-ignore lint/a11y/noLabelWithoutControl: 控件由 children 传入
    <label class="field">
      <span class="field__label">{props.label}</span>
      {props.children}
      {props.hint && <span class="field__hint">{props.hint}</span>}
    </label>
  );
}

/* ---------------- Notice ---------------- */
export function Notice(props: { tone?: "warn" | "danger" | "ok"; children: ComponentChildren }) {
  return (
    <div
      class={cx("notice", props.tone && `notice--${props.tone}`)}
      role={props.tone === "danger" ? "alert" : "note"}
    >
      {props.children}
    </div>
  );
}
