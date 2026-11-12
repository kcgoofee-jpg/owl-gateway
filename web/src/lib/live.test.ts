import { expect, it, vi } from "vitest";
import { liveSubscription } from "./live";

it("drops a pending poll when the stream recovers and releases it on unmount", async () => {
  vi.useFakeTimers();
  class Events {
    static current: Events;
    onopen = () => {};
    onmessage = (_: { data: string }) => {};
    onerror = () => {};
    close = vi.fn();
    constructor() {
      Events.current = this;
    }
  }
  vi.stubGlobal("EventSource", Events);
  let resolve!: (response: Response) => void;
  const fetch = vi.fn(
    () =>
      new Promise<Response>((done) => {
        resolve = done;
      }),
  );
  vi.stubGlobal("fetch", fetch);
  const live = liveSubscription<{ count: number }>("/stream", "/snapshot");
  const stop = live.start();
  const source = Events.current;
  try {
    source.onerror();
    expect(fetch).toHaveBeenCalledTimes(1);
    source.onopen();
    source.onmessage({ data: '{"count":2}' });
    resolve(new Response('{"count":1}'));
    await vi.advanceTimersByTimeAsync(5000);
    expect(live.data.value).toEqual({ count: 2 });
    expect(live.connected.value).toBe("sse");
    expect(fetch).toHaveBeenCalledTimes(1);
  } finally {
    stop();
    vi.useRealTimers();
    vi.unstubAllGlobals();
  }
  expect(source.close).toHaveBeenCalledOnce();
});
