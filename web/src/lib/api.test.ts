import { afterEach, expect, it, vi } from "vitest";
import { ApiError, sendJSON } from "./api";

afterEach(() => vi.unstubAllGlobals());
it("warns that a timed-out write may have succeeded without retrying it", async () => {
  const fetch = vi.fn().mockRejectedValue(new TypeError("network"));
  vi.stubGlobal("fetch", fetch);
  const error = await sendJSON("POST", "/synthetic", {}).catch((e: unknown) => e);
  expect(error).toBeInstanceOf(ApiError);
  expect((error as ApiError).message).toContain("可能已经生效");
  expect(fetch).toHaveBeenCalledTimes(1);
});
