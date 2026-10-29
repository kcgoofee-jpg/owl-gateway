/** workbench-config：预设组合、图参组装、五组批量展开、队列失败分类、模型限制。 */
import { describe, expect, it, vi } from "vitest";
import { DEFAULT_FORM } from "../../lib/nai-config";
import {
  applyModelModePrompt,
  type BatchGroup,
  batchTasks,
  buildTaskRequest,
  characterLimit,
  clampCount,
  clampRetryAfter,
  classifyFailure,
  composeNegative,
  composePositive,
  defaultGroups,
  directorRequest,
  enhanceLevel,
  enhanceScales,
  estimateAnlasCost,
  expandPromptRandomizer,
  MAX_BATCH_TASKS,
  modesFor,
  pendingVibeEncodes,
  prepareVibeRefs,
  qualityTags,
  randomCharacterPrompt,
  resolutionGroup,
  snapCharacterCenter,
  tokenLimit,
  ucOptions,
  upscaleRequest,
} from "./workbench-config";

describe("质量与负面预设", () => {
  it("兽人模式放在最终正向提示词最前面，并保留手写数据集前缀", () => {
    expect(applyModelModePrompt("nai-diffusion-5-full", "artist:x, owl, quality", "furry")).toBe(
      "fur dataset, artist:x, owl, quality",
    );
    expect(applyModelModePrompt("nai-diffusion-5-full", "background dataset, red owl", "furry")).toBe(
      "background dataset, red owl",
    );
    expect(applyModelModePrompt("nai-diffusion-3", "owl", "furry")).toBe("owl");
  });
  it("标准质量词按模型给；none 是空", () => {
    expect(qualityTags("nai-diffusion-4-5-full", "standard")).toBe("very aesthetic, masterpiece, no text");
    expect(qualityTags("nai-diffusion-4-5-full", "none")).toBe("");
  });

  it("V5 的「轻量」是官方短预设", () => {
    expect(qualityTags("nai-diffusion-5-full", "light")).toBe("very aesthetic, amazing quality, no text");
    expect(qualityTags("nai-diffusion-5-curated", "standard")).toBe("very aesthetic, masterpiece, no text");
    expect(qualityTags("nai-diffusion-4-5-full", "light")).toBe("very aesthetic, masterpiece, no text");
  });

  it("V5 引号文本自动移到最后的 teXt 区块，手写 Text 区块不重复转换", () => {
    expect(composePositive("nai-diffusion-5-full", '1girl holding "hello"', "", "standard")).toBe(
      "1girl holding, very aesthetic, masterpiece, no text\n\nteXt:\nhello",
    );
    expect(composePositive("nai-diffusion-5-full", "1girl, Text: hello", "", "standard")).toBe(
      "1girl, very aesthetic, masterpiece, no text\n\nText: hello",
    );
  });

  it("正面组合顺序：画师词 → 提示词 → 质量词", () => {
    expect(composePositive("nai-diffusion-4-5-full", "1girl", "artist:x", "standard")).toBe(
      "artist:x, 1girl, very aesthetic, masterpiece, no text",
    );
  });

  it("透明背景使用 V5 请求标记，保持 teXt 区块末尾不变", () => {
    const positive = composePositive(
      "nai-diffusion-5-full",
      'owl holding "hello"',
      "artist:x",
      "standard",
      true,
    );
    const body = buildTaskRequest(
      {
        form: { ...DEFAULT_FORM, model: "nai-diffusion-5-full", positive },
        mode: "generate",
        strength: 0,
        noise: 0,
        refs: [],
        transparentBackground: true,
        straightAlpha: true,
      },
      7,
    );
    const input = body.input as string;
    expect(input).toBe(positive);
    expect(input).toContain("transparent background");
    expect(input.indexOf("transparent background")).toBeLessThan(input.indexOf("teXt:"));
    expect(input.endsWith("teXt:\nhello")).toBe(true);
    expect((body.parameters as Record<string, unknown>).tag_hint_transparent_background).toBe(true);
    expect((body.parameters as Record<string, unknown>).straight_alpha).toBe(true);
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

  it("V4.5 Curated 的负面预设带人物重点，对齐官网 Human Focus", () => {
    expect(Object.keys(ucOptions("nai-diffusion-4-5-curated"))).toEqual(["none", "light", "heavy", "human"]);
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

describe("模型 token 上限与角色限制", () => {
  it("按模型族给上限：V5 / V4 系 / V3", () => {
    expect(tokenLimit("nai-diffusion-5-full")).toBe(1471);
    expect(tokenLimit("nai-diffusion-5-curated")).toBe(703);
    expect(tokenLimit("nai-diffusion-4-5-full")).toBe(512);
    expect(tokenLimit("nai-diffusion-3")).toBe(225);
  });

  it("V5 角色上限为 22，其余模型沿用 6", () => {
    expect(characterLimit("nai-diffusion-5-curated")).toBe(22);
    expect(characterLimit("nai-diffusion-4-5-full")).toBe(6);
  });

  it("旧模型角色坐标吸附到 5×5 网格，V5 保留自由坐标", () => {
    expect(snapCharacterCenter("nai-diffusion-4-5-full", { x: 0.25, y: 0.75 })).toEqual({ x: 0.3, y: 0.7 });
    expect(snapCharacterCenter("nai-diffusion-5-full", { x: 0.25, y: 0.75 })).toEqual({ x: 0.25, y: 0.75 });
  });
});

describe("官方绘图页参数", () => {
  it("V4 参考图先走编码，V3 保持原图", async () => {
    const encoded = await prepareVibeRefs(
      "nai-diffusion-4-5-full",
      [{ image: "raw", strength: 0.6, extracted: 1 }],
      async (image) => `enc:${image}`,
    );
    expect(encoded).toEqual([{ image: "enc:raw", strength: 0.6, extracted: 1 }]);
    const raw = await prepareVibeRefs(
      "nai-diffusion-3",
      [{ image: "raw", strength: 0.4 }],
      async () => "nope",
    );
    expect(raw).toEqual([{ image: "raw", strength: 0.4 }]);
    expect(
      await prepareVibeRefs("nai-diffusion-5-full", [{ image: "raw", strength: 1 }], async () => "nope"),
    ).toEqual([]);
  });

  it("V4 信息提取没改就复用编码，改了才重新请求", async () => {
    const encode = vi.fn(async (image: string) => `enc:${image}`);
    const reused = await prepareVibeRefs(
      "nai-diffusion-4-full",
      [{ image: "raw", strength: 0.6, extracted: 0.8, encoded: "enc:raw", encodedAt: 0.8 }],
      encode,
    );
    expect(reused).toEqual([{ image: "enc:raw", strength: 0.6, extracted: 0.8 }]);
    expect(encode).not.toHaveBeenCalled();
    const again = await prepareVibeRefs(
      "nai-diffusion-4-full",
      [{ image: "raw", strength: 0.6, extracted: 0.4, encoded: "enc:raw", encodedAt: 0.8 }],
      encode,
    );
    expect(again).toEqual([{ image: "enc:raw", strength: 0.6, extracted: 0.4 }]);
    expect(encode).toHaveBeenCalledTimes(1);
  });

  it("精准参考和角色提示词写进请求，并和 Vibe 互斥", () => {
    const body = buildTaskRequest(
      {
        form: DEFAULT_FORM,
        mode: "generate",
        strength: 0,
        noise: 0,
        refs: [{ image: "VIBE", strength: 0.5 }],
        precise: [{ image: "PNG", caption: "character", extracted: 1, strength: 0.8, fidelity: 0.4 }],
        characters: [{ prompt: "1girl", uc: "bad hands", center: { x: 0.2, y: 0.3 } }],
        positionMode: "custom",
      },
      7,
    );
    const p = body.parameters as Record<string, unknown>;
    expect(p.reference_image_multiple).toBeUndefined();
    expect(p.director_reference_images).toEqual(["PNG"]);
    expect(p.director_reference_strength_values).toEqual([0.8]);
    expect(p.director_reference_secondary_strength_values).toEqual([0.4]);
    expect(p.characterPrompts).toEqual([
      { prompt: "1girl", uc: "bad hands", center: { x: 0.3, y: 0.3 }, enabled: true },
    ]);
    expect(p.use_coords).toBe(true);
  });

  it("分辨率分组、Variety+、V3 SMEA 用 sm / sm_dyn", () => {
    expect(resolutionGroup(832, 1216)).toBe("normal");
    expect(resolutionGroup(100, 100)).toBe("custom");
    const smea = buildTaskRequest(
      {
        form: { ...DEFAULT_FORM, model: "nai-diffusion-3", smea: true },
        mode: "generate",
        strength: 0,
        noise: 0,
        refs: [],
        varietyPlus: true,
      },
      1,
    );
    const p = smea.parameters as Record<string, unknown>;
    expect(p.sm).toBe(true);
    expect(p.sm_dyn).toBe(true);
    expect(p.smea).toBeUndefined();
    expect(typeof p.skip_cfg_above_sigma).toBe("number");
    const v5Characters = buildTaskRequest(
      {
        form: { ...DEFAULT_FORM, model: "nai-diffusion-5-curated" },
        mode: "generate",
        strength: 0,
        noise: 0,
        refs: [],
        positionMode: "custom",
        characters: Array.from({ length: 23 }, (_, i) => ({
          prompt: `character ${i}`,
          uc: "",
          center: { x: 0.25, y: 0.75 },
        })),
      },
      1,
    );
    const v5Params = v5Characters.parameters as Record<string, unknown>;
    expect(v5Params.use_coords).toBe(true);
    expect(v5Params.characterPrompts).toHaveLength(22);
    expect((v5Params.characterPrompts as Array<{ center: { x: number; y: number } }>)[0]?.center).toEqual({
      x: 0.25,
      y: 0.75,
    });
    const v5 = buildTaskRequest(
      {
        form: { ...DEFAULT_FORM, model: "nai-diffusion-5-full", sampler: "k_euler_ancestral" },
        mode: "generate",
        strength: 0,
        noise: 0,
        refs: [],
      },
      1,
    );
    expect((v5.parameters as { prefer_brownian: boolean }).prefer_brownian).toBe(true);
  });

  it("导演表情写进 prompt，放大固定 2 倍", () => {
    expect(
      directorRequest({ image: "aa", tool: "emotion", prompt: "soft", emotion: "happy", defry: 9 }),
    ).toEqual({
      image: "aa",
      req_type: "emotion",
      prompt: "happy, soft",
      defry: 5,
    });
    expect(upscaleRequest("aa")).toMatchObject({ scale: 2, declared_blur_sigma: 0 });
    expect(enhanceScales(832, 1216)).toEqual([1]);
    expect(enhanceScales(640, 640)).toEqual([1, 1.5, 2]);
    expect(enhanceLevel(5)).toEqual({ strength: 0.7, noise: 0.1 });
  });
});

describe("随机器、角色开关、Anlas", () => {
  it("||选项|| 出图时取一项，并清掉多余逗号", () => {
    expect(expandPromptRandomizer("1girl, ||red|red|| hair")).toBe("1girl, red hair");
    expect(expandPromptRandomizer("a, ||,||, b")).toBe("a, b");
  });

  it("关掉的角色不进请求", () => {
    const body = buildTaskRequest(
      {
        form: { ...DEFAULT_FORM, model: "nai-diffusion-4-5-full" },
        mode: "generate",
        strength: 0,
        noise: 0,
        refs: [],
        characters: [
          { prompt: "girl", uc: "", enabled: false },
          { prompt: "boy", uc: "bad hands", enabled: true },
        ],
      },
      1,
    );
    expect((body.parameters as { characterPrompts: unknown }).characterPrompts).toEqual([
      { prompt: "boy", uc: "bad hands", enabled: true },
    ]);
  });

  it("规格内免费档 Anlas 为 0；待编码 Vibe +2", () => {
    const base = {
      width: 832,
      height: 1216,
      steps: 23,
      nSamples: 1,
      paid: false,
      pendingVibeEncodes: 0,
      vibeCount: 0,
      preciseCount: 0,
    };
    expect(estimateAnlasCost(base)).toBe(0);
    expect(estimateAnlasCost({ ...base, pendingVibeEncodes: 1 })).toBe(2);
    expect(estimateAnlasCost({ ...base, preciseCount: 1 })).toBe(5);
    expect(pendingVibeEncodes([{ extracted: 1 }, { extracted: 0.6, encodedAt: 0.6 }], true)).toBe(1);
    expect(pendingVibeEncodes([{ extracted: 1, encodedAt: 1 }], true)).toBe(0);
  });

  it("随机角色带性别词", () => {
    expect(randomCharacterPrompt("female").prompt).toMatch(/girl|1girl/);
    expect(randomCharacterPrompt("male").prompt).toMatch(/boy|1boy|man/);
  });
});
