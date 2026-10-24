import { render } from "preact";
import { act } from "preact/test-utils";
import { afterEach, expect, it, vi } from "vitest";
import { copyText } from "../../ui";
import { Gallery } from "./Gallery";
import data from "./gallery-data.json";

vi.mock("../../ui", () => ({ copyText: vi.fn() }));

const host = document.createElement("div");
afterEach(() => {
  render(null, host);
  vi.useRealTimers();
  vi.clearAllMocks();
});

it("loads only public official examples lazily without a referrer", () => {
  render(<Gallery />, host);
  const images = [...host.querySelectorAll("img")];
  expect(images.length).toBeGreaterThan(30);
  for (const image of images) {
    expect(image.src).toMatch(/^https:\/\/static\.novelai\.net\/quickstart\/v5\/\d+\.webp$/);
    expect(image.loading).toBe("lazy");
    expect(image.referrerPolicy).toBe("no-referrer");
  }
  expect(host.querySelector("#gallery")).not.toBeNull();
  expect(host.querySelector("h1, h2, h3, .lp-gallery__head, .lp-gallery__source-link")).toBeNull();
  // 常驻提示行已删：可点复制只靠图片自身的 title / aria-label
  expect(host.querySelector(".lp-gallery__hint")).toBeNull();
  expect(host.querySelectorAll(".lp-gallery > *")).toHaveLength(1);
});

it("keeps a usable source link when an external example fails", async () => {
  render(<Gallery />, host);
  const image = host.querySelector("img");
  const src = image?.getAttribute("src");
  await act(() => {
    image?.dispatchEvent(new Event("error"));
  });
  const fallback = host.querySelector(".lp-gallery__unavailable");
  expect(fallback?.textContent).toContain("图片暂不可用");
  expect(fallback?.getAttribute("href")).toBe(src);
});

it("copies the matching official prompt, shows feedback, then clears it", async () => {
  vi.useFakeTimers();
  vi.mocked(copyText).mockResolvedValue(true);
  render(<Gallery />, host);
  await act(async () => {
    host.querySelector("button")?.click();
  });
  expect(copyText).toHaveBeenCalledWith(data.images[0]?.prompt);
  expect(host.querySelector(".lp-gallery__feedback.is-visible")?.textContent).toBe("已复制提示词");
  await act(() => {
    vi.advanceTimersByTime(2000);
  });
  expect(host.querySelector(".lp-gallery__feedback.is-visible")).toBeNull();
});

it("reports rejected clipboard access without claiming a copy succeeded", async () => {
  vi.mocked(copyText).mockResolvedValue(false);
  render(<Gallery />, host);
  await act(async () => {
    host.querySelector("button")?.click();
  });
  expect(host.querySelector(".lp-gallery__feedback.is-visible")?.textContent).toBe("复制失败，请重试");
});
