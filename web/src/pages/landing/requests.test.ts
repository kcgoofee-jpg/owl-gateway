import { describe, expect, it } from "vitest";
import { modelLabel, okLine } from "./Requests";

describe("我的请求", () => {
  it("模型显示成短名", () => {
    expect(modelLabel({ model: "nai-diffusion-5-full", kind: "image" })).toBe("V5");
    expect(modelLabel({ model: "nai-diffusion-4-5-full", kind: "image" })).toBe("V4.5");
    expect(modelLabel({ model: "", kind: "tags" })).toBe("标签补全");
  });

  it("工具类请求显示工具名，不是模型名", () => {
    expect(modelLabel({ model: "nai-diffusion-4-5-full", kind: "vibe_encode" })).toBe("Vibe 编码");
    expect(modelLabel({ model: "nai-diffusion-4-5-full", kind: "upscale" })).toBe("放大");
    expect(okLine({ kind: "vibe_encode", anlas: 2, site_paid: false, message: "Vibe 编码 2 Anlas" })).toBe(
      "Vibe 编码  ·  扣 2 Anlas",
    );
  });

  it("成功的一条写清尺寸和怎么计的", () => {
    const base = { anlas: 0, site_paid: false, kind: "image_stream" };
    expect(okLine({ ...base, message: "832x1216/28step V5额度+1" })).toBe(
      "832 × 1216 / 28 步  ·  用 V5 额度",
    );
    expect(okLine({ ...base, message: "832x1216/28step 免费" })).toBe("832 × 1216 / 28 步  ·  free");
    expect(okLine({ ...base, anlas: 30, message: "832x1216/28step est=30A" })).toBe(
      "832 × 1216 / 28 步  ·  扣 30 Anlas",
    );
    expect(okLine({ anlas: 0, site_paid: true, kind: "image", message: "1216x832/28step; 站付V5 30A" })).toBe(
      "1216 × 832 / 28 步  ·  本站账号代付，不扣你的 Anlas",
    );
  });
});
