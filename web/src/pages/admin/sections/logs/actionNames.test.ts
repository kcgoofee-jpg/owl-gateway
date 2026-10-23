import { describe, expect, it } from "vitest";
import { actionKindGroups, actionLabel, actorFilter } from "./actionNames";

describe("actionLabel", () => {
  it("旧名并到新名，未知名原样返回", () => {
    expect(actionLabel("动态额度应用")).toBe("动态额度");
    expect(actionLabel("全站重置 Key")).toBe("全站杀头重置");
    expect(actionLabel("登录后台")).toBe("登录后台");
  });
});

describe("actionKindGroups", () => {
  it("按分组列出；同一显示名的多个旧名用 | 连成一个筛选值", () => {
    const g = actionKindGroups(["动态额度应用", "动态额度调整", "登录后台", "新建 Key"]);
    expect(g.map((x) => x.title)).toEqual(["后台 · 成员与 Key", "后台 · 设置", "系统自动"]);
    const sys = g.find((x) => x.title === "系统自动");
    expect(sys?.options).toEqual([{ value: "动态额度应用|动态额度调整", label: "动态额度" }]);
  });
  it("没归类的进「其他」，裸接口路径不列", () => {
    const g = actionKindGroups(["某个新操作", "POST /something", "登录后台"]);
    const other = g.find((x) => x.title === "其他");
    expect(other?.options.map((o) => o.label)).toEqual(["某个新操作"]);
  });
  it("没有任何操作时没有分组", () => {
    expect(actionKindGroups([])).toEqual([]);
  });
});

describe("actorFilter", () => {
  it("认手动 / 自动 / 成员，旧的 admin / system 并过去，别的不筛", () => {
    expect(actorFilter("manual")).toBe("manual");
    expect(actorFilter("auto")).toBe("auto");
    expect(actorFilter("member")).toBe("member");
    expect(actorFilter("admin")).toBe("manual");
    expect(actorFilter("system")).toBe("auto");
    expect(actorFilter("zzz")).toBe("");
    expect(actorFilter(undefined)).toBe("");
  });
});
