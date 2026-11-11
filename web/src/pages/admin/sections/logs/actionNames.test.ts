import { describe, expect, it } from "vitest";
import { actionKindGroups, actionLabel, actorFilter } from "./actionNames";

describe("actionLabel", () => {
  it("旧名并到新名，未知名原样返回", () => {
    expect(actionLabel("移除成员标签")).toBe("成员标签");
    expect(actionLabel("领取 Key（/register 直发）")).toBe("领取 Key");
    expect(actionLabel("登录后台")).toBe("登录后台");
  });
});

describe("actionKindGroups", () => {
  it("只按当前还会写的后台操作分组；旧名并进对应项，其余进「其他」", () => {
    const g = actionKindGroups([
      "领取 Key 失败（/register）",
      "领取 Key 失败（授权回调）",
      "登录后台",
      "新建 Key",
      "移除成员标签",
      "修改预算",
    ]);
    expect(g.map((x) => x.title)).toEqual(["后台 · 登录", "后台 · 成员与 Key", "后台 · 设置", "其他"]);
    const keys = g.find((x) => x.title === "后台 · 成员与 Key");
    expect(keys?.options).toEqual([
      { value: "新建 Key", label: "新建 Key" },
      { value: "移除成员标签", label: "成员标签" },
    ]);
    const other = g.find((x) => x.title === "其他");
    expect(other?.options).toEqual([
      { value: "领取 Key 失败（/register）|领取 Key 失败（授权回调）", label: "领取 Key 失败" },
    ]);
  });
  it("没归类的进「其他」，裸接口路径不列", () => {
    const g = actionKindGroups(["某个新操作", "POST /something", "登录后台"]);
    const other = g.find((x) => x.title === "其他");
    expect(other?.options.map((o) => o.label)).toEqual(["某个新操作"]);
  });
  it("已下线功能的动作名不进命名分组，只落在「其他」", () => {
    const g = actionKindGroups(["Anlas 核对", "清理日志"]);
    expect(g.map((x) => x.title)).toEqual(["其他"]);
    expect(new Set(g[0]?.options.map((o) => o.label))).toEqual(new Set(["Anlas 核对", "清理日志"]));
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
