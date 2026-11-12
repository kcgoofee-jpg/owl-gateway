import { describe, expect, it } from "vitest";
import { issueExplain, kindLabel, shortModel, statusHeadline, timing } from "./format";

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
  const features = [
    { id: "image", label: "文生图", short: "文生图" },
    { id: "augment", label: "导演工具（线稿、上色等）", short: "导演工具" },
  ];
  it("流式出图、导演工具等别名归到对应功能", () => {
    const kinds = { image_stream: "image", "augment-image": "augment" };
    expect(kindLabel("image_stream", features, kinds)).toBe("文生图");
    expect(kindLabel("augment-image", features, kinds)).toBe("导演工具");
    expect(kindLabel("chat", features, kinds)).toBe("文本 / 聊天");
    expect(kindLabel("mystery", features, kinds)).toBe("mystery");
    expect(kindLabel("", features, kinds)).toBe("—");
  });
});

describe("timing", () => {
  it("≥10 秒取整，等待不足 100ms 不显示", () => {
    expect(timing(50, 4200)).toEqual({ gen: "4.2 秒", wait: "" });
    expect(timing(12500, 15000)).toEqual({ gen: "15 秒", wait: "13 秒" });
    expect(timing(0, 0)).toEqual({ gen: "", wait: "" });
  });
});

describe("statusHeadline / issueExplain", () => {
  it("拒绝用原因，失败用上游分类，不再只写「错误」", () => {
    expect(statusHeadline({ status: "ok", reason_label: "", issue_label: "", up_status: 200 })).toBe("成功");
    expect(
      statusHeadline({ status: "rejected", reason_label: "排队满", issue_label: "", up_status: 0 }),
    ).toBe("排队满");
    expect(
      statusHeadline({
        status: "error",
        reason_label: "",
        issue_label: "出图通道接不了",
        up_status: 503,
      }),
    ).toBe("出图通道接不了");
    expect(statusHeadline({ status: "error", reason_label: "", issue_label: "", up_status: 503 })).toBe(
      "上游 503",
    );
  });
  it("展开说明不把请求形状当成原因", () => {
    expect(issueExplain("up_503")).toContain("请求形状");
    expect(issueExplain("up_422")).toContain("未知字段");
    expect(issueExplain("mystery")).toContain("未归类");
  });
});
