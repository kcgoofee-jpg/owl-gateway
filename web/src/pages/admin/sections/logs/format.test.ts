import { describe, expect, it } from "vitest";
import { autoReason, bugSource, kindLabel, shortModel, timing } from "./format";

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

describe("bugSource", () => {
  it("前缀类来源翻译", () => {
    expect(bugSource("upstream")).toBe("上游故障");
    expect(bugSource("maintenance:disk")).toBe("维护任务 · 磁盘监控");
    expect(bugSource("web:admin")).toBe("网页脚本 · 后台");
    expect(bugSource("maintenance:sync_roles")).toBe("维护任务 · 同步身份组");
    expect(bugSource("zzz")).toBe("zzz");
  });

  it("定时任务：job:<name> 显示标题，未知 name 原样显示", () => {
    expect(bugSource("job:quota")).toBe("定时任务 · 额度同步");
    expect(bugSource("job:upstream_snapshot")).toBe("定时任务 · 上游 V5 快照");
    expect(bugSource("job:hourly_cap")).toBe("定时任务 · 每小时上限自动调整");
    expect(bugSource("job:breaker")).toBe("定时任务 · 全站熔断");
    expect(bugSource("job:key_guard")).toBe("定时任务 · 单 Key 限流");
    expect(bugSource("job:maintenance")).toBe("定时任务 · 维护");
    expect(bugSource("job:idle_recycle")).toBe("定时任务 · 闲置回收");
    expect(bugSource("job:new_thing")).toBe("定时任务 · new_thing");
  });
});

describe("autoReason", () => {
  it("原因码 → 页面文案", () => {
    expect(autoReason("network")).toBe("网络断开，不算 Bug");
    expect(autoReason("upstream")).toBe("上游故障，不算 Bug");
    expect(autoReason("foreign_script")).toBe("非本站脚本");
    expect(autoReason("stale_warn")).toBe("24 小时未再出现");
    expect(autoReason("stale_error")).toBe("7 天未再出现");
  });
  it("不认识的码不留空", () => {
    expect(autoReason("something_new")).toBe("自动处理");
  });
});
