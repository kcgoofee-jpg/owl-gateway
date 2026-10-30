import { afterEach, describe, expect, it, vi } from "vitest";
import type { Shot } from "../pages/workbench/api";
import { historyRecordToShot, loadHistory, shotToHistoryRecord } from "./history-store";

const shot: Shot = {
  url: "data:image/png;base64,AQIDBA==",
  b64: "AQIDBA==",
  ext: "png",
  attrs: {
    model: "nai-diffusion-4-5-full",
    width: 832,
    height: 1216,
    steps: 23,
    scale: 5,
    sampler: "k_euler_ancestral",
    noise: "karras",
    seed: 42,
    prompt: "portrait",
    negative: "",
    seconds: 1.5,
  },
  label: "original",
};

afterEach(() => vi.unstubAllGlobals());

describe("IndexedDB image history records", () => {
  it("stores image bytes without duplicating data URLs and restores generation parameters", () => {
    const record = shotToHistoryRecord(shot);
    expect([...new Uint8Array(record.bytes)]).toEqual([1, 2, 3, 4]);
    expect(record).not.toHaveProperty("url");
    expect(record).not.toHaveProperty("b64");
    expect(historyRecordToShot(record)).toEqual(shot);
  });

  it("keeps repeated outputs unique even when they share a seed", () => {
    expect(shotToHistoryRecord(shot).id).not.toBe(shotToHistoryRecord(shot).id);
  });

  it("reports browsers without IndexedDB instead of claiming the history was saved", async () => {
    vi.stubGlobal("indexedDB", undefined);
    await expect(loadHistory()).rejects.toThrow("不支持历史持久化");
  });
});
