import { signal } from "@preact/signals";
import { afterEach, expect, it, vi } from "vitest";
import { getJSON } from "../../lib/api";
import { bootData } from "../../lib/boot";
import { createStore } from "./store";

vi.mock("../../lib/api", () => ({ getJSON: vi.fn() }));
vi.mock("../../lib/boot", () => ({ bootData: vi.fn() }));
vi.mock("../../lib/live", () => ({
  liveSubscription: () => ({ data: signal(null), connected: signal(true), start: () => () => {} }),
}));
afterEach(() => {
  vi.useRealTimers();
  vi.clearAllMocks();
});
it("refreshes while share-locked and stops rapid polling as soon as the server releases it", async () => {
  vi.useFakeTimers();
  vi.mocked(bootData).mockReturnValue({
    me: {
      logged_in: true,
      has_key: true,
      share_lock: { message: "待分享", channel_url: "https://discord.com" },
    },
    status: {},
  });
  vi.mocked(getJSON).mockResolvedValue({ logged_in: true, has_key: true });
  const store = createStore();
  const stop = store.start();
  await vi.advanceTimersByTimeAsync(5000);
  expect(getJSON).toHaveBeenCalledWith("/public/me", { cache: "no-store" });
  expect(store.me.value).not.toHaveProperty("share_lock");
  const count = vi.mocked(getJSON).mock.calls.length;
  await vi.advanceTimersByTimeAsync(5000);
  expect(getJSON).toHaveBeenCalledTimes(count);
  window.dispatchEvent(new Event("focus"));
  await Promise.resolve();
  expect(vi.mocked(getJSON).mock.calls.length).toBeGreaterThan(count);
  stop();
  const final = vi.mocked(getJSON).mock.calls.length;
  await vi.advanceTimersByTimeAsync(60000);
  window.dispatchEvent(new Event("focus"));
  expect(getJSON).toHaveBeenCalledTimes(final);
});
