/**
 * 总览 / 成员 / 日志 三个分区共用的小零件（组件库没有的：分段按钮、分页、下载、窗口宽度）。
 * 只用 ui 组件库和 tokens.css 的变量；样式在 shared.css（类名前缀 xa-）。
 */
import { ChevronLeft, ChevronRight } from "lucide-preact";
import type { ComponentChildren } from "preact";
import { useEffect, useId, useRef, useState } from "preact/hooks";
import { ApiError } from "../../../../lib/api";
import { Button, cx, toast } from "../../../../ui";
import { useAdmin } from "../../Admin";
import "./shared.css";

/* ---------------- hooks ---------------- */

/** 窗口宽度断点：只改布局（表格 ↔ 卡片），不改字号。 */
export function useMedia(query: string): boolean {
  const [match, setMatch] = useState(() => typeof matchMedia === "function" && matchMedia(query).matches);
  useEffect(() => {
    if (typeof matchMedia !== "function") return;
    const mq = matchMedia(query);
    const on = () => setMatch(mq.matches);
    on();
    mq.addEventListener("change", on);
    return () => mq.removeEventListener("change", on);
  }, [query]);
  return match;
}
/** 表格改成卡片的宽度。侧栏在 860px 以上常驻，内容区比视口窄约 240px，所以表格要到 1100px 以上才放得下。 */
export const NARROW = "(max-width: 1100px)";

export function useDebounced<T>(value: T, ms = 250): T {
  const [v, setV] = useState(value);
  useEffect(() => {
    const t = setTimeout(() => setV(value), ms);
    return () => clearTimeout(t);
  }, [value, ms]);
  return v;
}

/**
 * 带「正在换页」状态的取数：url 变化时旧数据保留（不闪烁），pending 为 true 直到新数据到达。
 * 仍然跟着全局刷新节拍。
 */
export function usePageData<T>(url: string): { data: T | null; error: string | null; pending: boolean } {
  const r = useAdmin<T>(url);
  const mark = useRef<{ url: string; data: T | null }>({ url, data: r.data });
  if (mark.current.url !== url) mark.current = { url, data: r.data };
  const pending = !r.error && (r.loading || r.data === mark.current.data);
  return { data: r.data, error: r.error, pending };
}

/** 未处理 Bug 数（日志页子导航的角标用）。侧栏「日志」角标由外壳的 BadgePoller 统一维护，这里只读。 */
export function useOpenBugs(): number {
  const { data } = useAdmin<{ open: number }>("/admin/api/errors");
  return data?.open ?? 0;
}

/* ---------------- 分段按钮 ---------------- */
export function Seg<T extends string>(props: {
  label: string;
  value: T;
  options: Array<{ id: T; label: ComponentChildren; title?: string; disabled?: boolean }>;
  onChange: (v: T) => void;
  class?: string;
}) {
  return (
    // biome-ignore lint/a11y/useSemanticElements: 分段按钮是一排并列按钮，用 fieldset 的默认版式不合适；role=group + aria-label 即标准写法
    <div class={cx("xa-seg", props.class)} role="group" aria-label={props.label}>
      {props.options.map((o) => (
        <button
          key={o.id}
          type="button"
          class="xa-seg__btn"
          aria-pressed={o.id === props.value}
          title={o.title}
          disabled={o.disabled}
          onClick={() => props.onChange(o.id)}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}

/* ---------------- 分页 ---------------- */
export function Pager(props: {
  page: number;
  pages: number;
  total: number;
  perPage: number;
  sizes?: number[];
  onPage: (p: number) => void;
  onSize?: (n: number) => void;
}) {
  const { page, pages, total, perPage } = props;
  const from = total ? (page - 1) * perPage + 1 : 0;
  const to = Math.min(total, page * perPage);
  const nums: Array<number | "gap-l" | "gap-r"> = [];
  for (let i = 1; i <= pages; i++) {
    if (i === 1 || i === pages || Math.abs(i - page) <= 1) {
      const last = nums[nums.length - 1];
      if (typeof last === "number" && i - last > 1) nums.push(i < page ? "gap-l" : "gap-r");
      nums.push(i);
    }
  }
  const jumpId = useId();
  return (
    <div class="xa-pager">
      <span class="xa-pager__info tabular">
        {from}–{to} / 共 {total}
      </span>
      {props.sizes && props.onSize && (
        <label class="xa-pager__size">
          每页
          <select
            class="select"
            value={perPage}
            onChange={(e) => props.onSize?.(Number((e.target as HTMLSelectElement).value))}
          >
            {props.sizes.map((s) => (
              <option key={s} value={s}>
                {s}
              </option>
            ))}
          </select>
        </label>
      )}
      <span class="spacer" />
      {pages > 1 && (
        <nav class="xa-pager__nav" aria-label="分页">
          <Button
            size="sm"
            icon
            aria-label="上一页"
            disabled={page <= 1}
            onClick={() => props.onPage(page - 1)}
          >
            <ChevronLeft size={16} aria-hidden="true" />
          </Button>
          {nums.map((n) =>
            typeof n === "number" ? (
              <Button
                key={n}
                size="sm"
                variant={n === page ? "primary" : "default"}
                aria-label={`第 ${n} 页`}
                aria-current={n === page ? "page" : undefined}
                onClick={() => props.onPage(n)}
              >
                {n}
              </Button>
            ) : (
              <span key={n} class="muted" aria-hidden="true">
                …
              </span>
            ),
          )}
          <Button
            size="sm"
            icon
            aria-label="下一页"
            disabled={page >= pages}
            onClick={() => props.onPage(page + 1)}
          >
            <ChevronRight size={16} aria-hidden="true" />
          </Button>
          {pages > 10 && (
            <label class="xa-pager__jump" for={jumpId}>
              前往
              <input
                id={jumpId}
                class="input"
                type="number"
                min={1}
                max={pages}
                inputMode="numeric"
                onKeyDown={(e) => {
                  if (e.key !== "Enter") return;
                  const v = Math.max(1, Math.min(pages, Number((e.target as HTMLInputElement).value) || 1));
                  props.onPage(v);
                }}
              />
              页
            </label>
          )}
        </nav>
      )}
    </div>
  );
}

/* ---------------- 零散 ---------------- */
export function Empty(props: { title: string; children?: ComponentChildren }) {
  return (
    <div class="xa-empty">
      <p class="xa-empty__title">{props.title}</p>
      {props.children && <p class="muted small">{props.children}</p>}
    </div>
  );
}

/** 一条用量小条：文字写明数字，条本身只是装饰。max=0 表示不限。 */
export function MiniBar(props: { label: string; used: number; max: number; tone?: "v5" | "warn" }) {
  const pct = props.max > 0 ? Math.min(100, (props.used / props.max) * 100) : 0;
  const full = props.max > 0 && props.used >= props.max;
  return (
    <div class="xa-minibar">
      <span class="xa-minibar__l">{props.label}</span>
      <span class="xa-minibar__track" aria-hidden="true">
        <span
          class={cx("xa-minibar__fill", props.tone && `is-${props.tone}`, full && "is-full")}
          style={{ width: `${pct}%` }}
        />
      </span>
      <span class="xa-minibar__n tabular">
        {props.used}
        <span class="muted"> / {props.max > 0 ? props.max : "不限"}</span>
      </span>
    </div>
  );
}

/* ---------------- 下载（带错误提示） ---------------- */
function filenameOf(res: Response, fallback: string): string {
  const cd = res.headers.get("Content-Disposition") ?? "";
  const star = /filename\*=UTF-8''([^;]+)/i.exec(cd);
  if (star?.[1]) {
    try {
      return decodeURIComponent(star[1]);
    } catch {
      /* 退回下一种写法 */
    }
  }
  const plain = /filename="?([^";]+)"?/i.exec(cd);
  return plain?.[1] ?? fallback;
}

/** 导出文件：失败时（如 429 导出太频繁）用 toast 显示后端给的原因，而不是跳到一页 JSON。 */
/**
 * 下载文件（带后台会话）。`onProgress` 收到 0–100 的百分比（服务端给了 Content-Length 才有；
 * 打包阶段服务端还没开始回数据时不会回调，调用方自己显示「打包中」）。
 */
export async function downloadFile(
  url: string,
  fallbackName: string,
  onProgress?: (pct: number) => void,
): Promise<void> {
  try {
    const res = await fetch(url, { credentials: "same-origin" });
    if (!res.ok) {
      let msg = `导出失败（HTTP ${res.status}）`;
      try {
        const b = (await res.json()) as { message?: string; detail?: string; error?: { message?: string } };
        msg = b.error?.message ?? b.message ?? b.detail ?? msg;
      } catch {
        /* 保持默认文案 */
      }
      throw new ApiError(res.status, msg);
    }
    const total = Number(res.headers.get("content-length") ?? 0);
    let blob: Blob;
    if (onProgress && total > 0 && res.body) {
      const reader = res.body.getReader();
      const parts: Uint8Array[] = [];
      let got = 0;
      for (;;) {
        const { done, value } = await reader.read();
        if (done) break;
        parts.push(value);
        got += value.length;
        onProgress(Math.min(100, Math.floor((got / total) * 100)));
      }
      blob = new Blob(parts as BlobPart[], { type: res.headers.get("content-type") ?? "" });
    } else {
      blob = await res.blob();
    }
    const a = document.createElement("a");
    a.href = URL.createObjectURL(blob);
    a.download = filenameOf(res, fallbackName);
    document.body.append(a);
    a.click();
    a.remove();
    setTimeout(() => URL.revokeObjectURL(a.href), 10_000);
  } catch (e) {
    toast(e instanceof Error ? e.message : String(e), { error: true });
  }
}

/**
 * 表单对话框的「防误关」：有未保存的修改时，Esc / 点遮罩不直接关，而是在框里问一句。
 * 原生 <dialog> 在 Esc 时已经关掉了，所以这里换 key 让 <Dialog> 重新挂载（表单状态在父组件里，不丢）。
 */
export function useCloseGuard(dirty: boolean, onClose: () => void) {
  const [gen, setGen] = useState(0);
  const [asking, setAsking] = useState(false);
  return {
    gen,
    asking,
    tryClose: () => {
      if (!dirty) return onClose();
      setAsking(true);
      setGen((g) => g + 1);
    },
    keep: () => setAsking(false),
    discard: onClose,
  };
}

/** 重置范围的勾选：V4.5 固定勾上，V5 和 Anlas 默认不勾（站长 10-18）。 */
export function ResetScopeChecks(props: {
  value: { v5: boolean; anlas: boolean };
  onChange: (v: { v5: boolean; anlas: boolean }) => void;
}) {
  const { value, onChange } = props;
  return (
    <fieldset class="xa-form__opts mb-fieldset">
      <legend class="small muted">重置哪些额度</legend>
      <label class="xa-check">
        <input type="checkbox" checked disabled />
        <span>V4.5（一定重置）</span>
      </label>
      <label class="xa-check">
        <input
          type="checkbox"
          checked={value.v5}
          onChange={(e) => onChange({ ...value, v5: e.currentTarget.checked })}
        />
        <span>V5</span>
      </label>
      <label class="xa-check">
        <input
          type="checkbox"
          checked={value.anlas}
          onChange={(e) => onChange({ ...value, anlas: e.currentTarget.checked })}
        />
        <span>Anlas</span>
      </label>
    </fieldset>
  );
}
