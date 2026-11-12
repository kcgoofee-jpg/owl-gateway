import { render } from "preact";
import { act } from "preact/test-utils";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

vi.mock("../../lib/api", async (original) => ({
  ...(await original<typeof import("../../lib/api")>()),
  getJSON: vi.fn(),
}));

import { getJSON } from "../../lib/api";
import { RequestStatsView, requestCategories } from "./RequestStats";
import { MyRequestsCard } from "./Requests";
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

/** 「我的请求」的卡片：拿 getJSON 的调用次数和面板内容判断「切 tab 有没有再发请求」。 */
function requestsPayload() {
  const row = (id: number, status: string) => ({
    id,
    rid: `r${id}`,
    ts: 1793833200 + id,
    kind: "image",
    model: "nai-diffusion-4-5-full",
    status,
    images: status === "ok" ? 1 : 0,
    anlas: 0,
    site_paid: false,
    wait_s: null,
    dur_s: null,
    reason: "",
    code: status === "ok" ? null : 429,
    message: status === "ok" ? "" : "太频繁",
    delivery_state: "unknown",
    settlement_status: null,
    remaining_anlas: 0,
    remaining_v5: 0,
    remaining_legacy: 0,
    app: "client",
  });
  return {
    items: [row(3, "ok"), row(2, "rejected"), row(1, "ok")],
    next: null,
    summary: { total: 3, ok: 2, rejected: 1, error: 0, cancelled: 0 },
    since: 1793833200,
  };
}

const tabButton = (name: string) =>
  [...host.querySelectorAll<HTMLButtonElement>(".tabs__tab")].find((b) =>
    b.textContent?.trim().startsWith(name),
  );

it("switching result tabs filters locally without another request", async () => {
  const payload = requestsPayload();
  vi.mocked(getJSON).mockImplementation(async (url: string) =>
    url.includes("request-stats") ? sample : payload,
  );
  await act(async () => render(<MyRequestsCard />, host));
  await vi.waitFor(() => expect(host.textContent).toContain("全部 3"));
  expect(getJSON).toHaveBeenCalledTimes(1);
  expect(host.querySelectorAll(".lp-req")).toHaveLength(3);

  // 成功 / 被拒 / 失败 都只是本地筛，请求数不再增加
  for (const [name, rows] of [
    ["成功", 2],
    ["被拒", 1],
    ["失败", 0],
    ["全部", 3],
  ] as const) {
    await act(async () => tabButton(name)?.click());
    expect(host.querySelectorAll(".lp-req")).toHaveLength(rows);
    expect(getJSON).toHaveBeenCalledTimes(1);
  }
});

it("loads the statistics once and keeps them when switching back", async () => {
  vi.mocked(getJSON).mockImplementation(async (url: string) =>
    url.includes("request-stats") ? sample : requestsPayload(),
  );
  await act(async () => render(<MyRequestsCard />, host));
  await vi.waitFor(() => expect(host.textContent).toContain("全部 3"));
  await act(async () => tabButton("统计")?.click());
  await vi.waitFor(() => expect(host.textContent).toContain("请求总数61次"));
  const statsCalls = () =>
    vi.mocked(getJSON).mock.calls.filter(([u]) => String(u).includes("request-stats")).length;
  expect(statsCalls()).toBe(1);

  // 切走再回来：不重读
  await act(async () => tabButton("全部")?.click());
  await act(async () => tabButton("统计")?.click());
  expect(statsCalls()).toBe(1);
  expect(host.textContent).toContain("请求总数61次");
});

it("offers retry after a statistics failure", async () => {
  vi.mocked(getJSON).mockImplementation(async (url: string) => {
    if (!String(url).includes("request-stats")) return requestsPayload();
    throw new Error("offline");
  });
  await act(async () => render(<MyRequestsCard />, host));
  await vi.waitFor(() => expect(host.textContent).toContain("全部 3"));
  await act(async () => tabButton("统计")?.click());
  await vi.waitFor(() => expect(host.querySelector('[role="alert"]')?.textContent).toContain("统计读取失败"));
  vi.mocked(getJSON).mockImplementation(async (url: string) =>
    String(url).includes("request-stats") ? sample : requestsPayload(),
  );
  await act(async () =>
    host.querySelector<HTMLButtonElement>('[role="alert"] button, .lp-request-stats__error button')?.click(),
  );
  await vi.waitFor(() => expect(host.textContent).toContain("请求总数61次"));
});
