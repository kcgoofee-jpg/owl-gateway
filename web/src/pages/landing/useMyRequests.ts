import { useCallback, useEffect, useRef, useState } from "preact/hooks";
import { ApiError, getJSON } from "../../lib/api";
import type { MyRequest, MyRequestStats, MyRequests } from "./types";

const REFRESH_MS = 60_000;

/**
 * 列表一次拉当天（本地筛 tab）；统计第一次点开才读。
 * 可见时 60 秒、聚焦时同一套时钟刷新：未翻页才重读列表，统计已打开才重读统计。
 */
export function useMyRequests() {
  const [rows, setRows] = useState<MyRequest[] | null>(null);
  const [next, setNext] = useState<number | null>(null);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState("");
  const [sum, setSum] = useState<MyRequests["summary"] | null>(null);
  const [stats, setStats] = useState<MyRequestStats | null>(null);
  const [statsErr, setStatsErr] = useState("");
  const paged = useRef(false);
  const statsOpened = useRef(false);
  const listSeq = useRef(0);
  const statsSeq = useRef(0);

  const loadList = useCallback(async (before?: number) => {
    const seq = ++listSeq.current;
    if (before) paged.current = true;
    setBusy(true);
    setErr("");
    try {
      const d = await getJSON<MyRequests>(`/public/me/requests${before ? `?before=${before}` : ""}`);
      if (seq !== listSeq.current) return;
      setRows((old) => (before && old ? [...old, ...d.items] : d.items));
      setNext(d.next);
      setSum(d.summary);
    } catch (e) {
      if (seq !== listSeq.current) return;
      setErr(e instanceof ApiError ? e.message : "读取失败，请稍后重试");
    } finally {
      if (seq === listSeq.current) setBusy(false);
    }
  }, []);

  const loadStats = useCallback(async () => {
    const seq = ++statsSeq.current;
    try {
      const result = await getJSON<MyRequestStats>("/public/me/request-stats");
      if (seq !== statsSeq.current) return;
      setStats(result);
      setStatsErr("");
      setSum(result.summary);
    } catch (e) {
      if (seq !== statsSeq.current) return;
      setStatsErr(e instanceof ApiError ? e.message : "统计读取失败，请稍后重试");
    }
  }, []);

  useEffect(() => {
    void loadList();
  }, [loadList]);

  useEffect(() => {
    const refresh = () => {
      if (document.visibilityState !== "visible") return;
      if (!paged.current) void loadList();
      if (statsOpened.current) void loadStats();
    };
    const timer = setInterval(refresh, REFRESH_MS);
    const onVis = () => document.visibilityState === "visible" && refresh();
    document.addEventListener("visibilitychange", onVis);
    window.addEventListener("focus", refresh);
    return () => {
      clearInterval(timer);
      document.removeEventListener("visibilitychange", onVis);
      window.removeEventListener("focus", refresh);
    };
  }, [loadList, loadStats]);

  const openStats = () => {
    if (statsOpened.current) return;
    statsOpened.current = true;
    void loadStats();
  };

  const retryStats = () => {
    setStats(null);
    setStatsErr("");
    void loadStats();
  };

  return {
    rows,
    next,
    busy,
    err,
    sum,
    stats,
    statsErr,
    openStats,
    retryStats,
    loadMore: () => void loadList(next ?? undefined),
  };
}
