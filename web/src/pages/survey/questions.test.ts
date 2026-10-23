import { describe, expect, it } from "vitest";
import { EMPTY, IMPROVE, payload, validate } from "./questions";

describe("问卷", () => {
  it("保留六个改进选项", () => {
    expect(IMPROVE).toHaveLength(6);
  });

  it("客户端必填", () => {
    expect(validate(EMPTY)).toEqual({ client: "第 1 题还没填" });
    expect(validate({ ...EMPTY, client: "   " })).toEqual({ client: "第 1 题还没填" });
    expect(validate({ ...EMPTY, client: "柏宝绘" })).toEqual({});
  });

  it("问卷不再提交捐赠或旧题字段", () => {
    const body = payload({
      client: " 柏宝绘 1.2 ",
      improve: ["稳定性"],
      improveOther: " 想要深色模式 ",
    });
    expect(body).toEqual({
      client: "柏宝绘 1.2",
      improve: ["稳定性"],
      improve_other: "想要深色模式",
    });
  });
});
