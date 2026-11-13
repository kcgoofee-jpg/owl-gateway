import { render } from "preact";
import { act } from "preact/test-utils";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

vi.mock("../../lib/api", async (original) => ({
  ...(await original<typeof import("../../lib/api")>()),
  getJSON: vi.fn(),
}));

import { getJSON } from "../../lib/api";
import { edits, getShared, refresh, tick, useAdmin } from "./data";

type Out = { data: { v: number } | null; error: string | null; loading: boolean };
let out: Out | null = null;
function Probe() {
  out = useAdmin<{ v: number }>("/cached-probe");
  return null;
}

beforeEach(() => {
  vi.mocked(getJSON).mockReset();
  tick.value = 0;
  edits.value = 0;
  out = null;
});
afterEach(() => {
  vi.restoreAllMocks();
});

it("manual refresh bumps the tick (no auto polling)", () => {
  const before = tick.value;
  refresh();
  expect(tick.value).toBe(before + 1);
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

it("useAdmin serves cached data on remount without refetching, refetches on manual refresh", async () => {
  vi.mocked(getJSON).mockResolvedValue({ v: 1 });
  const host = document.createElement("div");
  document.body.append(host);
  await act(async () => {
    render(<Probe />, host);
  });
  await act(async () => {});
  expect((out as Out).data).toEqual({ v: 1 });
  expect(getJSON).toHaveBeenCalledTimes(1);

  // 切 tab = 卸载再挂载：同一节拍命中缓存，秒显且不再发请求
  await act(async () => {
    render(null, host);
  });
  await act(async () => {
    render(<Probe />, host);
  });
  expect((out as Out).data).toEqual({ v: 1 });
  expect((out as Out).loading).toBe(false);
  expect(getJSON).toHaveBeenCalledTimes(1);

  // 手动刷新：节拍 +1，重取新数据
  vi.mocked(getJSON).mockResolvedValue({ v: 2 });
  await act(async () => {
    refresh();
  });
  await act(async () => {});
  expect((out as Out).data).toEqual({ v: 2 });
  expect(getJSON).toHaveBeenCalledTimes(2);

  render(null, host);
  host.remove();
});
