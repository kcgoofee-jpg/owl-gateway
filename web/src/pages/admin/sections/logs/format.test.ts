import { describe, expect, it } from "vitest";
import { kindLabel, shortModel, timing } from "./format";

describe("shortModel", () => {
  it("把 NovelAI 模型名缩成版本号", () => {
    expect(shortModel("nai-diffusion-5-full")).toBe("V5");
    expect(shortModel("nai-diffusion-4-5-curated")).toBe("V4.5 Curated");
    expect(shortModel("nai-diffusion-4-full")).toBe("V4");
    expect(shortModel("nai-diffusion-3-inpainting")).toBe("V3 重绘");
    expect(shortModel("")).toBe("");
    expect(shortModel("some-very-long-unknown-model-name")).toBe("some-very-long-u…");
  });
});

describe("kindLabel", () => {
  it("流式出图、导演工具等别名归到对应功能", () => {
    expect(kindLabel("image_stream")).toBe("文生图");
    expect(kindLabel("augment-image")).toBe("导演工具");
    expect(kindLabel("chat")).toBe("文本 / 聊天");
    expect(kindLabel("mystery")).toBe("mystery");
    expect(kindLabel("")).toBe("—");
  });
});

describe("timing", () => {
  it("≥10 秒取整，等待不足 100ms 不显示", () => {
    expect(timing(50, 4200)).toEqual({ gen: "4.2 秒", wait: "" });
    expect(timing(12500, 15000)).toEqual({ gen: "15 秒", wait: "13 秒" });
    expect(timing(0, 0)).toEqual({ gen: "", wait: "" });
  });
});
