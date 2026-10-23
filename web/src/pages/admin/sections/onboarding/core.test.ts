import { describe, expect, it } from "vitest";
import {
  extraChanged,
  extraRoleProblem,
  normalizeExtra,
  type RegistrationInfo,
  regBody,
  regServer,
  roleProblem,
} from "./core";

const labels = { image: "文生图", vibe: "Vibe 编码" };
const reg: RegistrationInfo = {
  configured: true,
  open: false,
  max_users: 10,
  features: ["image"],
  daily_images: 150,
  daily_v5: 6,
  image_scope: "all",
  expires_days: 30,
  role_guild: "",
  role_id: "",
  role_note: "",
  role_extra: [],
  active: 8,
};

describe("注册设置表单", () => {
  it("features 为 null 表示全部已开放功能", () => {
    expect(regServer({ ...reg, features: null }, labels)).toMatchObject({
      features_mode: "all",
      feat_image: true,
      feat_vibe: true,
    });
    expect(regServer(reg, labels)).toMatchObject({
      features_mode: "custom",
      feat_image: true,
      feat_vibe: false,
    });
  });
  it("请求体只含改过的字段", () => {
    const raw = () => "";
    expect(regBody({ max_users: 12 }, raw, ["image", "vibe"])).toEqual({ max_users: 12 });
  });
  it("功能清单成组提交：全部 → null，自定义 → 选中的名单", () => {
    const all = (n: string) => (n === "features_mode" ? "all" : true);
    expect(regBody({ features_mode: "all" }, all, ["image", "vibe"])).toEqual({ features: null });
    const custom = (n: string) => (n === "features_mode" ? "custom" : n === "feat_vibe");
    expect(regBody({ feat_vibe: true }, custom, ["image", "vibe"])).toEqual({ features: ["vibe"] });
  });
  it("身份组两个 ID 一起提交；不再带 notify（发名额公告已删除）", () => {
    const raw = (n: string) => (n === "role_guild" ? " 123456789012345678 " : "987654321098765432");
    expect(regBody({ role_guild: "123456789012345678" }, raw, [])).toEqual({
      role_guild: "123456789012345678",
      role_id: "987654321098765432",
    });
  });
  it("限定身份组必须同时填或同时留空", () => {
    expect(roleProblem("123", "")).not.toBeNull();
    expect(roleProblem("", "")).toBeNull();
    expect(roleProblem("1", "2")).toBeNull();
  });
});

describe("额外身份组（满足任意一个即可）", () => {
  const ok = { guild: "900000000000000222", role: "900000000000000333", note: "棱镜「创作者」" };
  it("ID 要是 15～21 位数字；空行忽略；最多 5 条", () => {
    expect(extraRoleProblem([])).toBeNull();
    expect(extraRoleProblem([ok, { guild: "", role: "", note: "" }])).toBeNull();
    expect(extraRoleProblem([{ ...ok, guild: "123" }])).toContain("第 1 条：服务器 ID");
    expect(extraRoleProblem([ok, { ...ok, role: "abc" }])).toContain("第 2 条：身份组 ID");
    expect(extraRoleProblem(Array.from({ length: 6 }, () => ok))).toContain("最多 5 条");
  });
  it("提交时去掉空行和首尾空白；改动判断忽略空行与空白", () => {
    expect(
      normalizeExtra([
        { ...ok, guild: ` ${ok.guild} ` },
        { guild: "", role: "", note: "" },
      ]),
    ).toEqual([ok]);
    expect(extraChanged([ok, { guild: "", role: "", note: "" }], [ok])).toBe(false);
    expect(extraChanged([{ ...ok, note: "别的" }], [ok])).toBe(true);
  });
});
