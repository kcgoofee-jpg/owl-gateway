import { signal } from "@preact/signals";

/**
 * 实时数据订阅：优先 SSE（服务端有变化才推），连不上时退回轮询（可见 1 秒 / 失败 5 秒 / 后台 10 秒，与旧版节奏一致）。
 * 返回的 signal 在任意组件里读取都会自动刷新。
 */
export function subscribe<T>(streamUrl: string, pollUrl: string, initial: T | null = null) {
  const data = signal<T | null>(initial);
  const connected = signal<"sse" | "poll" | "down">("down");
  let es: EventSource | null = null;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let failures = 0;

  const poll = async () => {
    clearTimeout(timer);
    try {
      const res = await fetch(pollUrl, {
        credentials: "same-origin",
        headers: { Accept: "application/json" },
      });
      if (!res.ok) throw new Error(String(res.status));
      data.value = (await res.json()) as T;
      connected.value = "poll";
      failures = 0;
    } catch {
      failures++;
      connected.value = "down";
    }
    const delay = document.hidden ? 10_000 : failures ? 5_000 : 1_000;
    timer = setTimeout(poll, delay);
  };

  const startSse = () => {
    if (typeof EventSource === "undefined") return poll();
    es = new EventSource(streamUrl, { withCredentials: true });
    es.onmessage = (e) => {
      try {
        data.value = JSON.parse(e.data) as T;
        connected.value = "sse";
      } catch {
        /* 忽略坏帧 */
      }
    };
    es.onerror = () => {
      // 浏览器会自动重连；连续失败（例如代理不支持 SSE）就退回轮询
      if (es && es.readyState === EventSource.CLOSED) {
        es = null;
        void poll();
      }
    };
  };

  startSse();
  return { data, connected };
}
