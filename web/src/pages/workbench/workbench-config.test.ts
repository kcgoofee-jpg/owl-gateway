/** workbench-config：预设组合、图参组装、五组批量展开、队列失败分类、token 粗估。 */
import { describe, expect, it } from "vitest";
import { DEFAULT_FORM } from "../../lib/nai-config";
import {
  type BatchGroup,
  batchTasks,
  buildTaskRequest,
  clampCount,
  clampRetryAfter,
  classifyFailure,
  composeNegative,
  composePositive,
  defaultGroups,
  estimateTokens,
  MAX_BATCH_TASKS,
  modesFor,
  qualityTags,
  tokenLimit,
  ucOptions,
} from "./workbench-config";

describe("质量与负面预设", () => {
  it("标准质量词按模型给；none 是空", () => {
    expect(qualityTags("nai-diffusion-4-5-full", "standard")).toBe("very aesthetic, masterpiece, no text");
    expect(qualityTags("nai-diffusion-4-5-full", "none")).toBe("");
  });

  it("V5 的「轻量」是官方短预设", () => {
    expect(qualityTags("nai-diffusion-5-full", "light")).toBe("very aesthetic, amazing quality, no text");
    expect(qualityTags("nai-diffusion-4-5-full", "light")).toBe("very aesthetic, masterpiece, no text");
  });

  it("正面组合顺序：画师词 → 提示词 → 质量词", () => {
    expect(composePositive("nai-diffusion-4-5-full", "1girl", "artist:x", "standard")).toBe(
      "artist:x, 1girl, very aesthetic, masterpiece, no text",
    );
  });

  it("负面组合：非 Curated 选预设且提示词没写 nsfw 时自动带 nsfw", () => {
    const neg = composeNegative("nai-diffusion-4-5-full", "", "light", "1girl");
    expect(neg.startsWith("nsfw, lowres")).toBe(true);
    const withNsfw = composeNegative("nai-diffusion-4-5-full", "", "light", "1girl, nsfw");
    expect(withNsfw.startsWith("nsfw")).toBe(false);
    const curated = composeNegative("nai-diffusion-4-5-curated", "", "light", "1girl");
    expect(curated.startsWith("nsfw")).toBe(false);
  });

  it("V3 不选预设且自己写了负面时，lowres 兜底让位；没写时补 lowres", () => {
    expect(composeNegative("nai-diffusion-3", "bad hands", "none", "1girl")).toBe("bad hands");
    expect(composeNegative("nai-diffusion-3", "", "none", "1girl")).toBe("lowres");
  });

  it("各模型都至少有无 / 轻量 / 强力 三档可选", () => {
    for (const m of [
      "nai-diffusion-5-full",
      "nai-diffusion-4-5-curated",
      "nai-diffusion-4-full",
      "nai-diffusion-furry-3",
    ]) {
      const keys = Object.keys(ucOptions(m));
      expect(keys).toContain("none");
      expect(keys).toContain("light");
      expect(keys).toContain("heavy");
    }
  });
});

describe("模式与图参", () => {
  it("policy.img2img 没开就只有文生图", () => {
    expect(modesFor(undefined)).toEqual(["generate"]);
    expect(modesFor({ img2img: false })).toEqual(["generate"]);
    expect(modesFor({ img2img: true })).toEqual(["generate", "img2img", "infill"]);
  });

  it("文生图：不塞图参", () => {
    const body = buildTaskRequest(
      { form: DEFAULT_FORM, mode: "generate", strength: 0.7, noise: 0, refs: [] },
      42,
    );
    expect(body.action).toBe("generate");
    const p = body.parameters as Record<string, unknown>;
    expect(p.image).toBeUndefined();
    expect(p.seed).toBe(42);
  });

  it("图生图：image + strength/noise；局部重绘：mask + img2img.strength + add_original_image=false", () => {
    const src = { name: "a.png", dataUrl: "data:", b64: "AAAA" };
    const i2i = buildTaskRequest(
      { form: DEFAULT_FORM, mode: "img2img", source: src, mask: null, strength: 0.6, noise: 0.1, refs: [] },
      1,
    );
    expect(i2i.action).toBe("img2img");
    const ip = i2i.parameters as Record<string, unknown>;
    expect(ip.image).toBe("AAAA");
    expect(ip.strength).toBe(0.6);
    expect(ip.noise).toBe(0.1);

    const infill = buildTaskRequest(
      {
        form: DEFAULT_FORM,
        mode: "infill",
        source: src,
        mask: { ...src, b64: "BBBB" },
        strength: 0.6,
        noise: 0,
        refs: [],
      },
      1,
    );
    expect(infill.action).toBe("infill");
    const fp = infill.parameters as Record<string, unknown>;
    expect(fp.mask).toBe("BBBB");
    expect(fp.img2img).toEqual({ strength: 0.6 });
    expect(fp.add_original_image).toBe(false);
  });

  it("参考图：三数组同发，强度逐张对应", () => {
    const body = buildTaskRequest(
      {
        form: DEFAULT_FORM,
        mode: "generate",
        strength: 0,
        noise: 0,
        refs: [
          { image: "R1", strength: 0.4 },
          { image: "R2", strength: 0.8 },
        ],
      },
      1,
    );
    const p = body.parameters as Record<string, unknown>;
    expect(p.reference_image_multiple).toEqual(["R1", "R2"]);
    expect(p.reference_information_extracted_multiple).toEqual([0.4, 0.8]);
    expect(p.reference_strength_multiple).toEqual([0.4, 0.8]);
  });
});

describe("五组批量", () => {
  /** 按序号改组的测试助手：躲开 noUncheckedIndexedAccess 的展开问题。 */
  const withGroups = (edits: Array<[number, Partial<BatchGroup>]>) => {
    const groups = defaultGroups();
    for (const [i, patch] of edits) {
      const g = groups[i];
      if (!g) throw new Error(`no group ${i}`);
      groups[i] = { ...g, ...patch };
    }
    return groups;
  };

  it("跳过没勾选 / 没写提示词的组；共用词与组内词按开关取", () => {
    const groups = withGroups([
      [0, { prompt: "a", count: 2 }],
      [1, { prompt: "b", enabled: false }],
      [2, { prompt: "c", artist: "artist:y", quality: "q", negative: "n" }],
    ]);
    const tasks = batchTasks(
      DEFAULT_FORM,
      { qualityPreset: "none", ucPreset: "none" },
      { artist: "artist:s", quality: "", negative: "" },
      // 画师词与负面词关掉共用：组内词生效
      { artist: false, quality: true, negative: false },
      groups,
    );
    expect(tasks.map((t) => t.label)).toEqual(["第 1 组 · 1/2", "第 1 组 · 2/2", "第 3 组 · 1/1"]);
    // 第 1 组没写组内画师词 → 不带画师词；第 3 组用组内的
    expect(tasks[0]?.form.positive).toBe("a");
    expect(tasks[2]?.form.positive).toBe("artist:y, c");
    // 第 3 组用组内负面词，而不是共用的
    expect(tasks[2]?.form.negative).toBe("n");
  });

  it("总量封顶 20 张", () => {
    const groups = withGroups([
      [0, { prompt: "a", count: 20 }],
      [1, { prompt: "b", count: 20 }],
    ]);
    const tasks = batchTasks(
      DEFAULT_FORM,
      { qualityPreset: "none", ucPreset: "none" },
      { artist: "", quality: "", negative: "" },
      { artist: true, quality: true, negative: true },
      groups,
    );
    expect(tasks).toHaveLength(MAX_BATCH_TASKS);
  });

  it("张数钳制 1..20", () => {
    expect(clampCount(0)).toBe(1);
    expect(clampCount(2.9)).toBe(2);
    expect(clampCount(99)).toBe(20);
  });
});

describe("队列失败分类", () => {
  it("429 且明确没开始画 → 可以重试；其余都停", () => {
    expect(classifyFailure(429, { retryable: true })).toBeNull();
    expect(classifyFailure(429, { retryable: true, executionUnknown: true })?.kind).toBe("unknown");
    expect(classifyFailure(429, { retryable: false })?.kind).toBe("failed");
    expect(classifyFailure(500, { message: "上游炸了" })).toEqual({ kind: "failed", message: "上游炸了" });
    expect(classifyFailure(403, {})?.message).toContain("403");
  });

  it("Retry-After 钳制：缺省 10 秒、下限 1、上限 3600", () => {
    expect(clampRetryAfter(null)).toBe(10);
    expect(clampRetryAfter(0.4)).toBe(1);
    expect(clampRetryAfter(5)).toBe(5);
    expect(clampRetryAfter(99999)).toBe(3600);
  });
});

describe("token 粗估", () => {
  it("按模型族给上限：V5 / V4 系 / V3", () => {
    expect(tokenLimit("nai-diffusion-5-full")).toBe(1471);
    expect(tokenLimit("nai-diffusion-4-5-full")).toBe(512);
    expect(tokenLimit("nai-diffusion-3")).toBe(225);
  });

  it("中日韩按字、英文按 4 字符 ≈ 1；权重写法不占", () => {
    expect(estimateTokens("abcdef")).toBe(2);
    expect(estimateTokens("1girl, seaside, sunset, very aesthetic")).toBe(10);
    expect(estimateTokens("猫头鹰")).toBe(3);
    // 1.2::tag:: 的权重前缀剥掉：只剩 tag 本体（8 字符 ≈ 2），不剥会是 4
    expect(estimateTokens("1.2::abcdef::")).toBe(2);
  });
});
