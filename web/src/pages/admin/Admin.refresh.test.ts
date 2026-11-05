import { afterEach, beforeEach, expect, it, vi } from "vitest";

vi.mock("../../lib/api", async (original) => ({
  ...(await original<typeof import("../../lib/api")>()),
  getJSON: vi.fn(),
}));

import { getJSON } from "../../lib/api";
import { busy, getShared, startTicker, tick } from "./Admin";
import { edits } from "./sections/live";

class Events {
  static OPEN = 1;
  static current: Events;
  readyState = 0;
  onmessage: (() => void) | null = null;
  onopen: (() => void) | null = null;
  onerror: (() => void) | null = null;
  constructor() {
    Events.current = this;
  }
  close() {
    this.readyState = 2;
  }
}
let stop: (() => void) | undefined;
beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(0);
  vi.stubGlobal("EventSource", Events);
  vi.spyOn(document, "hidden", "get").mockReturnValue(false);
  busy.value = 0;
  tick.value = 0;
});
afterEach(() => {
  stop?.();
  stop = undefined;
  vi.useRealTimers();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

it("coalesces initial SSE and visibility refresh, preserving busy protection", () => {
  stop = startTicker();
  const es = Events.current;
  es.readyState = Events.OPEN;
  es.onopen?.();
  es.onmessage?.();
  document.dispatchEvent(new Event("visibilitychange"));
  expect(tick.value).toBe(0);
  vi.advanceTimersByTime(15_000);
  expect(tick.value).toBe(1);
  busy.value = 1;
  es.onmessage?.();
  vi.advanceTimersByTime(15_000);
  expect(tick.value).toBe(1);
  busy.value = 0;
  expect(tick.value).toBe(2);
});

it("polls while SSE reconnects and removes all timers on unmount", () => {
  stop = startTicker();
  const es = Events.current;
  es.readyState = Events.OPEN;
  es.onopen?.();
  es.readyState = 0;
  es.onerror?.();
  vi.advanceTimersByTime(15_000);
  expect(tick.value).toBe(1);
  stop();
  stop = undefined;
  expect(vi.getTimerCount()).toBe(0);
  stop = startTicker();
  stop();
  stop = undefined;
  expect(vi.getTimerCount()).toBe(0);
});

it("shares an unfinished read across ticks but not across writes", async () => {
  const fetch = vi.mocked(getJSON);
  let resolve!: (value: unknown) => void;
  const pending = new Promise((done) => {
    resolve = done;
  });
  fetch.mockReturnValueOnce(pending).mockResolvedValueOnce({ fresh: true });
  const first = getShared("/slow-read", 1);
  expect(getShared("/slow-read", 2)).toBe(first);
  edits.value++;
  const afterWrite = getShared("/slow-read", 3);
  expect(afterWrite).not.toBe(first);
  resolve({ old: true });
  await Promise.all([first, afterWrite]);
  expect(getShared("/slow-read", 3)).toBe(afterWrite);
});

it("releases old filtered URLs instead of retaining every log page", async () => {
  vi.mocked(getJSON).mockResolvedValue({});
  const first = getShared("/bounded/0", 8);
  await first;
  for (let i = 1; i <= 32; i++) await getShared(`/bounded/${i}`, 8);
  expect(getShared("/bounded/0", 8)).not.toBe(first);
});
