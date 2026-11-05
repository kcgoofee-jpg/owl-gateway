import { render } from "preact";
import { act } from "preact/test-utils";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

vi.mock("../../lib/api", async (original) => ({
  ...(await original<typeof import("../../lib/api")>()),
  getJSON: vi.fn(),
}));

import { getJSON } from "../../lib/api";
import { RequestStatsPanel, RequestStatsView, requestCategories } from "./RequestStats";
import type { MyRequestStats } from "./types";

const sample: MyRequestStats = {
  since: 1793833200,
  summary: { total: 61, ok: 58, rejected: 1, error: 1, cancelled: 1 },
  images: 111,
  anlas: 167,
  avg_wait_s: 0,
  avg_dur_s: 4.1,
  uncertain_requests: 1,
  breakdown: [
    { kind: "image", model: "nai-diffusion-5-full", requests: 30 },
    { kind: "image_stream", model: "nai-diffusion-5-medium", requests: 30 },
    { kind: "vibe_encode", model: "nai-diffusion-5-full", requests: 1 },
  ],
};
let host: HTMLDivElement;
beforeEach(() => {
  vi.spyOn(document, "visibilityState", "get").mockReturnValue("visible");
  host = document.createElement("div");
  document.body.append(host);
});
afterEach(() => {
  act(() => render(null, host));
  host.remove();
  vi.useRealTimers();
  vi.restoreAllMocks();
  vi.resetAllMocks();
});

it("separates images, requests, cancelled outcomes and uncertain reservations", () => {
  act(() => render(<RequestStatsView data={sample} />, host));
  expect(host.textContent).toContain("95.1%");
  expect(host.textContent).toContain("111");
  expect(host.textContent).toContain("已取消1 次");
  expect(host.textContent).toContain("平均排队0秒");
  expect(host.textContent).toContain("1 次请求仍有执行结果待核对");
  expect(requestCategories(sample.breakdown)).toEqual([
    ["V5", 60],
    ["Vibe 编码", 1],
  ]);
});

it("shows an empty day and unknown timings without inventing a success rate", () => {
  act(() =>
    render(
      <RequestStatsView
        data={{
          ...sample,
          summary: { total: 0, ok: 0, rejected: 0, error: 0, cancelled: 0 },
          images: 0,
          anlas: 0,
          avg_wait_s: null,
          avg_dur_s: null,
          uncertain_requests: 0,
          breakdown: [],
        }}
      />,
      host,
    ),
  );
  expect(host.textContent).toContain("本服务日暂无请求");
  expect(host.textContent).toContain("成功率—");
  expect(host.textContent).toContain("平均排队—");
});

it("refreshes on a minute and visibility return, skips hidden pages and cleans up", async () => {
  vi.useFakeTimers();
  let visible = "visible";
  vi.spyOn(document, "visibilityState", "get").mockImplementation(() => visible as DocumentVisibilityState);
  vi.mocked(getJSON).mockResolvedValue(sample);
  const summary = vi.fn();
  await act(async () => render(<RequestStatsPanel onSummary={summary} />, host));
  expect(getJSON).toHaveBeenCalledTimes(1);
  await act(async () => {
    vi.advanceTimersByTime(60_000);
  });
  expect(getJSON).toHaveBeenCalledTimes(2);
  visible = "hidden";
  await act(async () => {
    vi.advanceTimersByTime(60_000);
  });
  expect(getJSON).toHaveBeenCalledTimes(2);
  visible = "visible";
  await act(async () => {
    document.dispatchEvent(new Event("visibilitychange"));
  });
  expect(getJSON).toHaveBeenCalledTimes(3);
  expect(summary).toHaveBeenLastCalledWith(sample.summary);
  act(() => render(null, host));
  expect(vi.getTimerCount()).toBe(0);
});

it("offers retry after failure and ignores a response after unmount", async () => {
  vi.mocked(getJSON).mockRejectedValueOnce(new Error("offline")).mockResolvedValueOnce(sample);
  const summary = vi.fn();
  await act(async () => render(<RequestStatsPanel onSummary={summary} />, host));
  await vi.waitFor(() => expect(host.querySelector('[role="alert"]')?.textContent).toContain("统计读取失败"));
  await act(async () => host.querySelector("button")?.click());
  await vi.waitFor(() => expect(host.textContent).toContain("请求总数61次"));
  act(() => render(null, host));
  let done: (data: MyRequestStats) => void = () => {};
  vi.mocked(getJSON).mockImplementation(
    () =>
      new Promise((resolve) => {
        done = resolve;
      }),
  );
  summary.mockClear();
  vi.mocked(getJSON).mockClear();
  await act(async () => render(<RequestStatsPanel onSummary={summary} />, host));
  await vi.waitFor(() => expect(getJSON).toHaveBeenCalledTimes(1));
  act(() => render(null, host));
  await act(async () => done(sample));
  expect(summary).not.toHaveBeenCalled();
});
