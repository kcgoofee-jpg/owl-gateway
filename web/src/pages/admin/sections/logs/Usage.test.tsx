import { render } from "preact";
import { act } from "preact/test-utils";
import { afterEach, describe, expect, it, vi } from "vitest";

const loc = vi.hoisted(() => ({
  path: "/admin/logs/usage",
  route: vi.fn(),
  query: {} as Record<string, string>,
}));

vi.mock("preact-iso", async (orig) => ({
  ...(await orig<typeof import("preact-iso")>()),
  useLocation: () => loc,
}));

vi.mock("../../data", () => ({
  edits: { value: 0 },
  useAdmin: (url: string | null) => {
    if (!url) return { data: null, error: null, loading: false };
    if (url.startsWith("/admin/api/members")) {
      return {
        data: {
          members: [
            {
              id: 12,
              name: "m12",
              enabled: true,
              discord_id: "1",
              discord: {
                id: "1",
                username: "user12",
                display_name: "测试员",
                avatar_url: "",
                profile_url: "",
              },
              all_key_ids: [12],
              key_count: 1,
              tags: [],
              policy: { defaults: {}, overrides: {}, effective: {} },
              today: { images: 0, legacy_free_images: 0, v5: 0, anlas: 0, requests: 0 },
              week: { images: 0, v5: 0, anlas: 0, requests: 0 },
              total_images: 0,
            },
          ],
        },
        error: null,
        loading: false,
      };
    }
    if (url.startsWith("/admin/api/features")) {
      return {
        data: { features: [{ id: "image", label: "文生图" }], kinds: { image: "image" } },
        error: null,
        loading: false,
      };
    }
    if (url.startsWith("/admin/api/logs")) {
      return {
        data: {
          page: 1,
          per_page: 20,
          total: 2,
          pages: 1,
          summary: { total: 2, ok: 0, rejected: 0, error: 2, cancelled: 0 },
          reason_counts: [],
          error_counts: [
            { code: "up_503", label: "出图通道接不了", count: 1 },
            { code: "up_422", label: "参数不被接受", count: 1 },
          ],
          logs: [
            {
              id: 2,
              ts: 1,
              key_id: 12,
              key_name: "m12",
              kind: "image",
              model: "nai-diffusion-4-5-curated",
              status: "error",
              images: 0,
              anlas: 0,
              tokens: 0,
              unconfirmed_anlas: 0,
              detail: "upstream 503（nai-diffusion-4-5-curated · 1024x1024）",
              reason: "",
              reason_label: "",
              wait_ms: 0,
              dur_ms: 2800,
              up_status: 503,
              rid: "b28fc8db",
              src: "",
              client: "Mozilla/5.0",
              token_id: "token-a",
              issue: "up_503",
              issue_label: "出图通道接不了",
              issue_hint: "上游 503",
            },
            {
              id: 1,
              ts: 1,
              key_id: 12,
              key_name: "m12",
              kind: "image",
              model: "nai-diffusion-5-full",
              status: "error",
              images: 0,
              anlas: 0,
              tokens: 0,
              unconfirmed_anlas: 0,
              detail: "upstream 422：未知字段：foo。",
              reason: "",
              reason_label: "",
              wait_ms: 0,
              dur_ms: 100,
              up_status: 422,
              rid: "aaaa1111",
              src: "",
              client: "",
              token_id: null,
              issue: "up_422",
              issue_label: "参数不被接受",
              issue_hint: "未知字段：foo。",
            },
          ],
        },
        error: null,
        loading: false,
      };
    }
    return { data: null, error: null, loading: false };
  },
}));

import { Usage } from "./Usage";

let host: HTMLElement | null = null;
afterEach(() => {
  if (host) render(null, host);
  host?.remove();
  host = null;
  loc.query = {};
  loc.route.mockClear();
});

describe("请求日志页", () => {
  const chips = () => [...document.querySelectorAll(".lg-chips .lg-chip")].map((el) => el.textContent);

  it("默认只给结果芯片，进入错误才展开上游错误分类；行上写清原因，名字跳到成员", async () => {
    host = document.createElement("div");
    document.body.append(host);
    await act(() => {
      render(<Usage />, host as HTMLElement);
    });
    // 行上直接写清原因，不用展开
    expect(host.textContent).toContain("出图通道接不了");
    expect(host.textContent).toContain("参数不被接受");
    expect(host.textContent).toContain("未知字段：foo。");
    const member = host.querySelector("a.lg-who") as HTMLAnchorElement | null;
    expect(member?.getAttribute("href")).toBe("/admin/members?open=12");
    // 默认视图只有「结果」一排；错误分类要点进「错误」才出现
    expect(chips().some((t) => t?.includes("错误"))).toBe(true);
    expect(chips().some((t) => t?.includes("出图通道接不了"))).toBe(false);

    render(null, host);
    loc.query = { status: "error" };
    await act(() => {
      render(<Usage />, host as HTMLElement);
    });
    expect(chips().some((t) => t?.includes("出图通道接不了"))).toBe(true);
  });
});
