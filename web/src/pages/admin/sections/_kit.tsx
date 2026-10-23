/**
 * 分区通用小部件：页头、指标格、加载 / 错误占位，以及几个各分区都要用的零件
 * （编辑期间暂停刷新、确认框、可滚动的表格外框、头像）。各分区只用这些 + ui 组件库。
 * 样式在 members/shared.css（类名前缀 xa-）。
 */
import type { ComponentChildren } from "preact";
import { useEffect, useState } from "preact/hooks";
import { Badge, Button, Card, cx, Dialog, Field, Notice, Skeleton, toast } from "../../../ui";
import { busy } from "../Admin";
import { call } from "./members/actions";
import "./members/shared.css";

export function SectionHead(props: { title: string; desc?: ComponentChildren; actions?: ComponentChildren }) {
  return (
    <header class="section-head">
      <div>
        <h1>{props.title}</h1>
        {props.desc && <p>{props.desc}</p>}
      </div>
      <span class="spacer" />
      {props.actions}
    </header>
  );
}

/** 「手动」/「自动」标签（站长 10-20：站长和 Claude 的改动算手动，定时任务和自动反应算自动）。 */
const KIND_TAGS = {
  manual: { label: "手动", tone: "accent", title: "站长或 Claude 做的改动" },
  auto: { label: "自动", tone: "info", title: "定时任务或自动反应做的改动" },
} as const;

export function KindTag(props: { kind: "manual" | "auto" }) {
  const t = KIND_TAGS[props.kind];
  return (
    <Badge tone={t.tone} title={t.title}>
      {t.label}
    </Badge>
  );
}

export function Stat(props: {
  label: ComponentChildren;
  value: ComponentChildren;
  hint?: ComponentChildren;
}) {
  return (
    <div class="stat">
      <div class="stat__label">{props.label}</div>
      <div class="stat__value">{props.value}</div>
      {props.hint && <div class="stat__hint">{props.hint}</div>}
    </div>
  );
}

export function Loading(props: { rows?: number }) {
  return (
    <div class="stack" aria-busy="true">
      {Array.from({ length: props.rows ?? 3 }, (_, i) => (
        <Skeleton key={i} h="56px" />
      ))}
    </div>
  );
}

export function LoadError(props: { error: string }) {
  return <Notice tone="danger">加载失败：{props.error}</Notice>;
}

/** 卡片还没拿到数据时的整张占位：加载中显示骨架，否则显示错误。 */
export function CardPending(props: {
  title: string;
  id?: string;
  loading: boolean;
  error: string | null | undefined;
}) {
  return (
    <Card {...(props.id ? { id: props.id } : {})} title={props.title}>
      {props.loading ? <Loading rows={2} /> : <LoadError error={props.error ?? "没有数据"} />}
    </Card>
  );
}

/** 本地分页：先显示 page 条，底下一个「显示更多（还有 N 条）」按钮。必须在早退（加载中 / 出错）之前调用。 */
export function useShowMore<T>(rows: T[], page = 50): { shown: T[]; more: ComponentChildren } {
  const [limit, setLimit] = useState(page);
  const shown = rows.slice(0, limit);
  const more = rows.length > shown.length && (
    <div class="lg-more">
      <Button onClick={() => setLimit(limit + page)}>显示更多（还有 {rows.length - shown.length} 条）</Button>
    </div>
  );
  return { shown, more };
}

/* ---------------- 编辑期间暂停自动刷新 ---------------- */
/** active 为真期间让全局刷新节拍暂停（Admin.tsx 的 busy 计数），避免把正在填的内容冲掉。组件挂载期间有效。 */
export function useBusy(active = true): void {
  useEffect(() => {
    if (!active) return;
    busy.value++;
    return () => {
      busy.value--;
    };
  }, [active]);
}

/* ---------------- 确认框 ---------------- */
/** 确认框里的一个动作：返回字符串 = 失败原因（显示在框里，保持打开）；返回 undefined = 成功，自动调 onClose。 */
type DialogAction = () => Promise<string | undefined> | string | undefined;

/**
 * 给确认框用的写操作：成功就 toast 并返回 undefined；失败返回原因，由确认框显示在框里
 * （原生 <dialog> 在最上层，会盖住 toast，所以错误必须显示在框里）。成功后会触发一次全局刷新。
 */
export async function actOrError<T = unknown>(
  method: "POST" | "PUT" | "PATCH" | "DELETE",
  url: string,
  body: unknown,
  okText: string | ((data: T) => string),
): Promise<string | undefined> {
  const r = await call<T>(method, url, body);
  if (!r.ok) return r.error;
  toast(typeof okText === "function" ? okText(r.data) : okText);
  return undefined;
}

/**
 * 缺省挂载即打开（父组件用 `{target && <ConfirmDialog …/>}` 控制）；也可以传 open 自己开关。
 * onConfirm 见 DialogAction；extra 是取消与确认之间的一个选项按钮（例如「仅保存，不公告」），走同一套等待 / 报错。
 */
export function ConfirmDialog(props: {
  open?: boolean;
  title: ComponentChildren;
  children: ComponentChildren;
  confirmText: string;
  danger?: boolean;
  /** 需要手动输入的确认词（高危操作） */
  phrase?: string;
  extra?: { text: string; onClick: DialogAction };
  onConfirm: DialogAction;
  onClose: () => void;
}) {
  const open = props.open ?? true;
  useBusy(open);
  const [pending, setPending] = useState(false);
  const [err, setErr] = useState("");
  const [typed, setTyped] = useState("");
  useEffect(() => {
    if (open) {
      setTyped("");
      setErr("");
      setPending(false);
    }
  }, [open]);
  const ok = !props.phrase || typed.trim() === props.phrase;
  const run = async (action: DialogAction) => {
    if (pending || !ok) return;
    setPending(true);
    setErr("");
    try {
      const r = await action();
      if (r === undefined) props.onClose();
      else setErr(r);
    } finally {
      setPending(false);
    }
  };
  const extra = props.extra;
  return (
    <Dialog
      open={open}
      title={props.title}
      onClose={props.onClose}
      footer={
        <>
          <Button onClick={props.onClose} disabled={pending}>
            取消
          </Button>
          {extra && (
            <Button disabled={pending || !ok} onClick={() => void run(extra.onClick)}>
              {extra.text}
            </Button>
          )}
          <Button
            variant={props.danger ? "danger" : "primary"}
            disabled={pending || !ok}
            onClick={() => void run(props.onConfirm)}
          >
            {pending ? "处理中…" : props.confirmText}
          </Button>
        </>
      }
    >
      <div class="xa-confirm">
        {props.children}
        {err && (
          <p class="xa-formerr" role="alert">
            {err}
          </p>
        )}
        {props.phrase && (
          <Field label={`请输入「${props.phrase}」确认`}>
            <input
              class="input"
              value={typed}
              autoComplete="off"
              onInput={(e) => setTyped((e.target as HTMLInputElement).value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") {
                  e.preventDefault();
                  void run(props.onConfirm);
                }
              }}
            />
          </Field>
        )}
      </div>
    </Dialog>
  );
}

/* ---------------- 表格外框 ---------------- */
/**
 * 表格的滚动外框：表头粘在框顶（sticky），内容多时框内滚动，窄屏只在框内横向滚动、不撑出页面。
 * 可滚动区域必须能用键盘聚焦（axe: scrollable-region-focusable），所以给 tabindex 和名称。
 */
export function TableWrap(props: { label: string; class?: string | undefined; children: ComponentChildren }) {
  return (
    // biome-ignore lint/a11y/noNoninteractiveTabindex: 可滚动区域需要键盘可聚焦，才能用方向键滚动
    <section class={cx("table-wrap xa-tbl", props.class)} tabIndex={0} aria-label={props.label}>
      {props.children}
    </section>
  );
}

/* ---------------- 头像 ---------------- */
/** 头像首字：名字里第一个汉字 / 假名 / 英文字母 / 数字；名字是「(๑•̀ㅂ•́)」这类颜文字时改用用户名。 */
const INITIAL = /[\p{Script=Han}\p{Script=Hiragana}\p{Script=Katakana}A-Za-z0-9]/u;
export function initialOf(name: string, fallback?: string | null): string {
  return (name.match(INITIAL)?.[0] ?? fallback?.match(INITIAL)?.[0] ?? "?").toUpperCase();
}

/** 有 Discord 头像地址（只认官方 CDN）就显示，加载失败退回首字母。 */
export function Avatar(props: {
  name: string;
  fallback?: string | null | undefined;
  src?: string | null | undefined;
  size?: number;
}) {
  const [failed, setFailed] = useState(false);
  const size = props.size ?? 32;
  const initial = initialOf(props.name, props.fallback);
  const ok = props.src?.startsWith("https://cdn.discordapp.com/") && !failed;
  return (
    <span class="xa-avatar" style={{ width: `${size}px`, height: `${size}px` }} aria-hidden="true">
      {initial}
      {ok && (
        <img
          src={props.src ?? ""}
          alt=""
          width={size}
          height={size}
          loading="lazy"
          referrerpolicy="no-referrer"
          onError={() => setFailed(true)}
        />
      )}
    </span>
  );
}
