/** nai-config.ts：请求体组装、免费档钳制、种子、标签补全解析、入口闸门文案。 */
import { describe, expect, it } from "vitest";
import {
  BASE_MODELS,
  buildRequest,
  clampForm,
  costHint,
  DEFAULT_FORM,
  entryProblem,
  type PaintForm,
  parseSuggestions,
  promptProblem,
  resolveSeed,
  samplersFor,
  V5_MODELS,
} from "./nai-config";

const form = (patch: Partial<PaintForm> = {}): PaintForm => ({ ...DEFAULT_FORM, ...patch });

describe("buildRequest", () => {
  it("默认表单是一张 832x1216 / 28 步 / 单张的 V4.5 请求，负面词放 negative_prompt", () => {
    const body = buildRequest(form({ positive: "1girl, sunset", negative: " lowres " }), 1234);
    expect(body).toEqual({
      input: "1girl, sunset",
      model: "nai-diffusion-4-5-full",
      action: "generate",
      parameters: {
        width: 832,
        height: 1216,
        steps: 28,
        scale: 5,
        sampler: "k_euler_ancestral",
        n_samples: 1,
        seed: 1234,
        negative_prompt: "lowres",
        noise_schedule: "karras",
        cfg_rescale: 0,
        params_version: 3,
      },
    });
  });

  it("V3 不带噪声调度和 params_version，SMEA 打开时带 smea / smea_dyn", () => {
    const body = buildRequest(form({ model: "nai-diffusion-3", smea: true }), 7);
    const p = body.parameters as Record<string, unknown>;
    expect(p.noise_schedule).toBeUndefined();
    expect(p.params_version).toBeUndefined();
    expect(p.smea).toBe(true);
    expect(p.smea_dyn).toBe(true);
  });

  it("种子缺省时自己随机，且落在上游接受的 0..4294967295 里", () => {
    const p = buildRequest(form()).parameters as Record<string, unknown>;
    expect(typeof p.seed).toBe("number");
    expect(p.seed).toBeGreaterThanOrEqual(0);
    expect(p.seed).toBeLessThanOrEqual(4294967295);
  });
});

describe("resolveSeed / samplers", () => {
  it("空和 -1 都随机；好输入原样保留；越界退回随机", () => {
    const lo = resolveSeed("");
    const hi = resolveSeed("4294967295");
    expect(Number.isInteger(lo) && lo >= 0).toBe(true);
    expect(hi).toBe(4294967295);
    expect(resolveSeed("-2")).toBeGreaterThanOrEqual(0);
    expect(resolveSeed("abc")).toBeGreaterThanOrEqual(0);
  });

  it("V3 与 V4 系的采样器分组不同，V5 用 V4 那组", () => {
    expect(samplersFor("nai-diffusion-3")).toContain("k_lms");
    expect(samplersFor("nai-diffusion-4-5-full")).not.toContain("k_lms");
    expect(samplersFor("nai-diffusion-5-full")).toEqual(samplersFor("nai-diffusion-4-5-full"));
  });
});

describe("clampForm（免费档）", () => {
  it("免费 Key：步数压到 28、超大自定义尺寸等比缩小，并给出说明", () => {
    const { form: f, notes } = clampForm(form({ steps: 40, width: 2048, height: 2048 }), false);
    expect(f.steps).toBe(28);
    expect(f.width * f.height).toBeLessThanOrEqual(1024 * 1024);
    expect(notes.some((n) => n.includes("28 步"))).toBe(true);
    expect(notes.some((n) => n.includes("尺寸"))).toBe(true);
  });

  it("免费 Key：预设尺寸本来就合规，不出说明；付费 Key 原样放行", () => {
    expect(clampForm(form(), false).notes).toEqual([]);
    const big = form({ steps: 50, width: 2048, height: 2048 });
    const paid = clampForm(big, true);
    expect(paid.form).toEqual(big);
    expect(paid.notes).toEqual([]);
  });
});

describe("parseSuggestions", () => {
  it("认字符串数组和 {tag} / {name} 对象数组，去重并截到 12 条", () => {
    expect(parseSuggestions({ tags: ["1girl", { tag: "sunset" }, { name: "seaside" }, "1girl"] })).toEqual([
      "1girl",
      "sunset",
      "seaside",
    ]);
    expect(parseSuggestions({ tags: Array.from({ length: 20 }, (_, i) => `t${i}`) })).toHaveLength(12);
  });

  it("认不出的形状回空数组，不让界面崩", () => {
    expect(parseSuggestions({ tags: "nope" })).toEqual([]);
    expect(parseSuggestions(null)).toEqual([]);
    expect(parseSuggestions({})).toEqual([]);
  });
});

describe("提示与闸门", () => {
  it("空提示词拦下；有提示词放行", () => {
    expect(promptProblem(form({ positive: "  " }))).toBeTruthy();
    expect(promptProblem(form({ positive: "1girl" }))).toBeNull();
  });

  it("没拿到 Key / 网页没登录 / 出图未开放，各自的文案", () => {
    expect(entryProblem({})).toContain("刷新页面");
    expect(entryProblem({ key: "k", requires_web_login: true })).toContain("Discord 登录");
    expect(entryProblem({ key: "k", policy: { ...defaults, image: false } })).toContain("没有开放");
    expect(entryProblem({ key: "k", policy: defaults })).toBeNull();
  });

  it("计费提示按模型与权限走", () => {
    expect(costHint("nai-diffusion-5-full", false)).toContain("V5");
    expect(costHint("nai-diffusion-4-5-full", false)).toContain("免费");
    expect(costHint("nai-diffusion-4-5-full", true)).toContain("Anlas");
  });

  it("模型清单里 V5 单独一组，基础组里没有 V5", () => {
    expect(BASE_MODELS.some((m) => m.id.startsWith("nai-diffusion-5"))).toBe(false);
    expect(V5_MODELS.map((m) => m.id)).toEqual(["nai-diffusion-5-full"]);
  });
});

const defaults = {
  daily_images: 100,
  daily_v5: 0,
  daily_anlas: 0,
  monthly_anlas: 0,
  rpm: 10,
  image: true,
  v5: false,
  vibe: false,
  tags: false,
  upscale: false,
  augment: false,
  img2img: false,
  paid_parameters: false,
};
