/** 日志页的筛选条件存在地址栏查询串里：可以收藏、分享、从别的页面（成员页「生成记录」、总览功能名）直接跳进来。 */
import { useLocation } from "preact-iso";

export function useQueryState(): {
  query: Record<string, string>;
  /** 改查询串（值为 null / 空串 = 删掉这一项）；用 replace，不往历史里堆 */
  set: (patch: Record<string, string | null>) => void;
} {
  const { path, query, route } = useLocation();
  return {
    query,
    set: (patch) => {
      // 筛选框的延迟回调（输入防抖等）可能在人已经点去别的栏目之后才触发：地址已经不是这一页，就不要再跳回来
      if (typeof location !== "undefined" && location.pathname !== path) return;
      const next = new URLSearchParams(query);
      for (const [k, v] of Object.entries(patch)) {
        if (v === null || v === "") next.delete(k);
        else next.set(k, v);
      }
      const qs = next.toString();
      route(qs ? `${path}?${qs}` : path, true);
    },
  };
}

/** 页码 / 每页条数：非法值回到默认 */
export function intParam(v: string | undefined, fallback: number, allowed?: number[]): number {
  const n = Number(v);
  if (!Number.isInteger(n) || n < 1) return fallback;
  if (allowed && !allowed.includes(n)) return fallback;
  return n;
}
