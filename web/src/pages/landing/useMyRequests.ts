import { useCallback, useEffect, useRef, useState } from "preact/hooks";
import { ApiError, getJSON, isAbortError } from "../../lib/api";
import type { MyRequest, MyRequestStats, MyRequests } from "./types";

const INTERVAL_MS = 60_000;

/**
 * 「我的请求」的数据：当天列表一次拉齐、本地筛 tab；统计第一次点开才读。
 * 可见时 60 秒刷新一次（翻过页就停列表刷新，避免打乱正在看的位置）。
 */
export function useMyRequests() {
  const [rows, setRows] = useState<MyRequest[] | null>(null);
  const [next, setNext] = useState<number | null>(null);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState("");
  const [sum, setSum] = useState<MyRequests["summary"] | null>(null);
  const [stats, setStats] = useState<MyRequestStats | null>(null);
  const [statsErr, setStatsErr] = useState("");
  const [statsOpened, setStatsOpened] = useState(false);
  const paged = useRef(false);
  const statsOpenedRef = useRef(false);
  const listAbort = useRef<AbortController | null>(null);
  const statsAbort = useRef<AbortController | null>(null);
  statsOpenedRef.current = statsOpened;

  const loadList = useCallback(async (before?: number) => {
    listAbort.current?.abort();
    const ac = new AbortController();
    listAbort.current = ac;
    setBusy(true);
    setErr("");
    if (before) paged.current = true;
    try {
      const d = await getJSON<MyRequests>(`/public/me/requests${before ? `?before=${before}` : ""}`, {
        signal: ac.signal,
      });
      if (ac.signal.aborted) return;
      setRows((old) => (before && old ? [...old, ...d.items] : d.items));
      setNext(d.next);
      setSum(d.summary);
    } catch (e) {
      if (isAbortError(e) || ac.signal.aborted) return;
      setErr(e instanceof ApiError ? e.message : "读取失败，请稍后重试");
    } finally {
      if (listAbort.current === ac) setBusy(false);
    }
  }, []);

  const loadStats = useCallback(async () => {
    if (document.visibilityState === "hidden") return;
    statsAbort.current?.abort();
    const ac = new AbortController();
    statsAbort.current = ac;
    try {
      const result = await getJSON<MyRequestStats>("/public/me/request-stats", { signal: ac.signal });
      if (ac.signal.aborted) return;
      setStats(result);
      setStatsErr("");
      setSum(result.summary);
    } catch (e) {
      if (isAbortError(e) || ac.signal.aborted) return;
      setStatsErr(e instanceof ApiError ? e.message : "统计读取失败，请稍后重试");
    }
  }, []);

  useEffect(() => {
    void loadList();
    return () => {
      listAbort.current?.abort();
      statsAbort.current?.abort();
    };
  }, [loadList]);

  useEffect(() => {
    if (statsOpened) void loadStats();
  }, [statsOpened, loadStats]);

  useEffect(() => {
    const tick = () => {
      if (document.visibilityState !== "visible") return;
      if (!paged.current) void loadList();
      if (statsOpenedRef.current) void loadStats();
    };
    const timer = setInterval(tick, INTERVAL_MS);
    const onVis = () => document.visibilityState === "visible" && tick();
    document.addEventListener("visibilitychange", onVis);
    window.addEventListener("focus", tick);
    return () => {
      clearInterval(timer);
      document.removeEventListener("visibilitychange", onVis);
      window.removeEventListener("focus", tick);
    };
  }, [loadList, loadStats]);

  const openStats = useCallback(() => setStatsOpened(true), []);
  const retryStats = useCallback(() => {
    setStats(null);
    setStatsErr("");
    void loadStats();
  }, [loadStats]);
  const loadMore = useCallback(() => {
    if (next == null) return;
    void loadList(next);
  }, [loadList, next]);

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
    loadMore,
  };
}
