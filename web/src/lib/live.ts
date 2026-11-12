import { signal } from "@preact/signals";
import { getJSON } from "./api";

/** 启动和重连时用轮询兜底。SSE 连上后停掉当前轮询。 */
export function connectEvents(
  url: string,
  message: (event: MessageEvent) => void,
  startPolling: () => () => void,
) {
  let source: EventSource;
  let stopPolling: (() => void) | undefined;
  const fallback = () => {
    stopPolling ??= startPolling();
  };
  try {
    source = new EventSource(url, { withCredentials: true });
  } catch {
    return startPolling();
  }
  const startup = setTimeout(fallback, 3000);
  source.onmessage = message;
  source.onerror = fallback;
  source.onopen = () => {
    clearTimeout(startup);
    stopPolling?.();
    stopPolling = undefined;
  };
  return () => {
    clearTimeout(startup);
    source.onmessage = source.onerror = source.onopen = null;
    source.close();
    stopPolling?.();
  };
}

export function liveSubscription<T>(streamUrl: string, pollUrl: string, initial: T | null = null) {
  const data = signal<T | null>(initial);
  const connected = signal<"sse" | "poll" | "down">("down");
  const startPolling = () => {
    const controller = new AbortController();
    let timer: ReturnType<typeof setTimeout> | undefined;
    const poll = async () => {
      try {
        const next = await getJSON<T>(pollUrl, { signal: controller.signal });
        if (controller.signal.aborted) return;
        data.value = next;
        connected.value = "poll";
      } catch {
        if (controller.signal.aborted) return;
        connected.value = "down";
      }
      timer = setTimeout(poll, document.hidden ? 10_000 : connected.value === "down" ? 5_000 : 1_000);
    };
    void poll();
    return () => {
      controller.abort();
      clearTimeout(timer);
    };
  };
  const start = () =>
    connectEvents(
      streamUrl,
      (event) => {
        try {
          data.value = JSON.parse(event.data) as T;
          connected.value = "sse";
        } catch {
          /* 忽略坏帧 */
        }
      },
      startPolling,
    );
  return { data, connected, start };
}
