/**
 * 列表「看的时候冻结」（做法参考 Grafana 实时日志、LiteLLM 请求日志、Logz.io：有人在看就暂停，
 * 新数据攒着，给一个「有 N 条新记录」的入口）：
 * - 没在看（列表顶部在视口里、没展开行、没开大图、在第 1 页）：跟着全局刷新节拍实时更新；
 * - 在看：列表的顺序和条目都冻住，已有的行原地更新数值；新出现的行只计数，点提示条才放进来；
 * - 自己在后台做了操作（act / call）：下一份数据直接放进来，不然刚删掉的行还挂着；
 * - 换筛选 / 换页（key 变了）：直接用新数据。
 */
import { signal } from "@preact/signals";
import type { RefObject } from "preact";
import { useEffect, useRef, useState } from "preact/hooks";
import { Button } from "../../../ui";

/** 后台自己的写操作次数（act / call 成功后 +1）：冻结中的列表见到它变了就放行下一份数据。 */
export const edits = signal(0);

type Snap<T> = {
  key: string;
  rows: T[];
  /** 记快照时的写操作次数 */
  edits: number;
  /** 写操作之后还没换掉的那份旧数据（等它被新数据替换才算「操作结果到了」） */
  stale?: T[];
};

/**
 * latest：这次取到的完整列表（null = 还没取到）；key：筛选 / 页码等，变了就不冻结；
 * pending：换页 / 换筛选的请求还没回来（这时 latest 还是旧条件的数据，不能记成新 key 的快照）。
 */
export function useFrozenList<T>(
  latest: T[] | null | undefined,
  idOf: (row: T) => string | number,
  opts: { frozen: boolean; key: string; pending?: boolean },
): { rows: T[] | null; fresh: number; changed: boolean; apply: () => void } {
  const snap = useRef<Snap<T> | null>(null);
  const [, force] = useState(0);
  const e = edits.value;
  const none = { fresh: 0, changed: false, apply: () => {} };
  if (!latest) return { rows: snap.current?.rows ?? null, ...none };
  const s = snap.current;
  const take = () => {
    snap.current = { key: opts.key, rows: latest, edits: e };
  };
  if (!s || !opts.frozen) {
    if (!opts.pending || !s) take();
    return { rows: latest, ...none };
  }
  if (s.key !== opts.key) {
    if (!opts.pending) take();
    return { rows: latest, ...none };
  }
  if (s.edits !== e) {
    // 自己刚做了操作：等这份旧数据被换掉（操作结果到了），再整份放进来
    s.stale ??= latest;
    if (latest !== s.stale) take();
    return { rows: latest, ...none };
  }
  // 冻结：顺序和条目照旧，数值换成最新的；新冒出来的只计数
  const byId = new Map(latest.map((r) => [idOf(r), r] as const));
  const shown = new Set(s.rows.map(idOf));
  const rows = s.rows.map((r) => byId.get(idOf(r)) ?? r);
  const fresh = latest.reduce((n, r) => n + (shown.has(idOf(r)) ? 0 : 1), 0);
  const changed =
    fresh > 0 || latest.length !== s.rows.length || latest.some((r, i) => idOf(r) !== idOf(s.rows[i] as T));
  const apply = () => {
    take();
    force((n) => n + 1);
  };
  return { rows, fresh, changed, apply };
}

/** 列表顶部被滚出视口（往下看了）就算「在看」。窗口滚动驱动，后台主区域就是整页滚动。 */
export function useScrolledPast(ref: RefObject<HTMLElement | null>, margin = 8): boolean {
  const [past, setPast] = useState(false);
  useEffect(() => {
    let raf = 0;
    const check = () => {
      raf = 0;
      const el = ref.current;
      setPast(!!el && el.getBoundingClientRect().top < -margin);
    };
    const onScroll = () => {
      if (!raf) raf = requestAnimationFrame(check);
    };
    check();
    window.addEventListener("scroll", onScroll, { passive: true, capture: true });
    window.addEventListener("resize", onScroll);
    return () => {
      window.removeEventListener("scroll", onScroll, { capture: true });
      window.removeEventListener("resize", onScroll);
      if (raf) cancelAnimationFrame(raf);
    };
  }, [ref, margin]);
  return past;
}

/**
 * 冻结期间攒下的新数据入口：贴在列表上方（吸顶），点一下放进来并回到列表顶部。
 * fresh > 0 写「有 N 条新记录」；只是顺序 / 状态变了写 changedText。
 */
export function FreshBar(props: {
  fresh: number;
  changed: boolean;
  onApply: () => void;
  unit?: string;
  changedText?: string;
  anchor?: RefObject<HTMLElement | null>;
}) {
  if (!props.changed) return null;
  const text =
    props.fresh > 0 ? `有 ${props.fresh} ${props.unit ?? "条"}新记录` : (props.changedText ?? "列表有更新");
  return (
    <div class="xa-fresh" role="status">
      <Button
        size="sm"
        variant="primary"
        onClick={() => {
          props.onApply();
          props.anchor?.current?.scrollIntoView({ block: "start", behavior: "smooth" });
        }}
      >
        ↑ {text}，点击查看
      </Button>
    </div>
  );
}
