import { describe, expect, it } from "vitest";
import {
  allPromptText,
  aspectOf,
  charsText,
  hasPicture,
  infoLine,
  negativeText,
  paramsLine,
  parseExtra,
  positiveText,
  statusText,
  statusTone,
} from "./auditData";

describe("parseExtra", () => {
  it("坏 JSON / 空 / 非对象都当作空", () => {
    expect(parseExtra("")).toEqual({});
    expect(parseExtra(null)).toEqual({});
    expect(parseExtra("{坏")).toEqual({});
    expect(parseExtra("123")).toEqual({});
    expect(parseExtra('{"params":{"seed":7}}')).toEqual({ params: { seed: 7 } });
  });
});

describe("aspectOf / paramsLine", () => {
  it("按宽高算比例，缺失按 2:3", () => {
    expect(aspectOf({ params: { width: 1216, height: 832 } })).toBeCloseTo(1.4615, 3);
    expect(aspectOf({})).toBeCloseTo(2 / 3, 5);
    expect(aspectOf({ params: { width: 0, height: 10 } })).toBeCloseTo(2 / 3, 5);
  });
  it("参数摘要只列有值的项，种子 0 也要显示；采样器枚举显示成友好名", () => {
    expect(
      paramsLine({ params: { seed: 0, sampler: "k_euler", steps: 28, scale: 5, width: 832, height: 1216 } }),
    ).toBe("种子 0 · Euler · 28 步 · CFG 5 · 832×1216");
    expect(paramsLine({ params: { sampler: "k_dpmpp_2m" } })).toBe("DPM++ 2M");
    expect(paramsLine({ params: { sampler: "k_euler_ancestral" } })).toBe("Euler A");
    // 没见过的枚举原样显示
    expect(paramsLine({ params: { sampler: "k_something_new" } })).toBe("k_something_new");
    expect(paramsLine({})).toBe("");
  });
});

describe("提示词块", () => {
  it("角色提示词与负面提示词按编号合并", () => {
    expect(charsText(["a", "b"])).toBe("角色1：a\n角色2：b");
    expect(charsText(undefined)).toBe("");
    expect(negativeText({ negative: "bad" }, { char_negatives: ["x"] })).toBe("bad\n角色1负面：x");
    expect(negativeText({ negative: "" }, {})).toBe("");
  });
});

describe("大图下方一行与复制文本", () => {
  it("编号 · 参数 · 带秒的时间 · 成员；没有编号的老记录不写编号", () => {
    const ex = { params: { seed: 7, sampler: "k_euler", steps: 28, scale: 5, width: 832, height: 1216 } };
    const line = infoLine({ rid: "edae7aa9", ts: 1_800_000_000, key_name: "月见" }, ex);
    expect(line).toMatch(
      /^编号 edae7aa9 · 种子 7 · Euler · 28 步 · CFG 5 · 832×1216 · \d{2}-\d{2} \d{2}:\d{2}:\d{2} · 月见$/,
    );
    expect(infoLine({ rid: "", ts: 1_800_000_000, key_name: "" }, {})).toMatch(
      /^\d{2}-\d{2} [\d:]+ · 已删除的 Key$/,
    );
  });
  it("复制正面提示词带上角色提示词", () => {
    expect(positiveText({ prompt: "1girl" }, { char_prompts: ["a"] })).toBe("1girl\n角色1：a");
    expect(
      allPromptText({ prompt: "1girl", negative: "bad" }, { char_prompts: ["a"], char_negatives: ["x"] }),
    ).toBe("1girl\n角色1：a\n\nbad\n角色1负面：x");
    expect(positiveText({ prompt: "" }, {})).toBe("");
  });
});

describe("其他", () => {
  it("是否有图、状态文字", () => {
    expect(hasPicture({ has_image: 0, has_thumb: 1 })).toBe(true);
    expect(hasPicture({ has_image: false, has_thumb: false })).toBe(false);
    expect(statusText("ok")).toBe("成功");
    expect(statusText("timeout")).toBe("timeout");
    expect(statusText("")).toBe("错误");
    expect(statusTone("ok")).toBe("ok");
    expect(statusTone("rejected")).toBe("warn");
    expect(statusTone("error")).toBe("danger");
  });
});
