import { render } from "preact";
import { act } from "preact/test-utils";
import { afterEach, expect, it, vi } from "vitest";
import { NewApiCredentials } from "./Topbar";

const host = document.createElement("div");
afterEach(() => {
  render(null, host);
  vi.unstubAllGlobals();
});

it("keeps the login link stable and renders only login credentials", async () => {
  vi.stubGlobal(
    "fetch",
    vi.fn().mockResolvedValue({
      ok: true,
      json: async () => ({
        username: "fixture-user",
        password: "fixture-password",
        key: "excluded-token",
        balance_usd: 5,
      }),
    }),
  );
  await act(async () => {
    render(<NewApiCredentials />, host);
  });
  await vi.waitFor(() => expect(host.querySelector("a")).not.toBeNull());
  const link = host.querySelector("a");
  expect(link?.href).toBe("https://newapi.davidzhao.top/");
  expect(host.textContent).toContain("用户名");
  expect(host.textContent).toContain("密码");
  expect(host.textContent).not.toMatch(/excluded-token|余额|签到|api key|base url/);
  await act(async () => {
    render(<NewApiCredentials />, host);
  });
  expect(host.querySelector("a")).toBe(link);
});
