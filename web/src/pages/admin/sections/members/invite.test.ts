import { describe, expect, it } from "vitest";
import { inviteMessage } from "./invite";

describe("inviteMessage", () => {
  const msg = inviteMessage("https://owl.example/", "nai-abc123");
  it("包含 Key 与站点地址", () => {
    expect(msg).toContain("nai-abc123");
    expect(msg).toContain("https://owl.example/");
  });
  it("说明网页用 Discord 登录，不再有粘贴 Key 登录", () => {
    expect(msg).toContain("用 Discord 登录");
    expect(msg).toContain("不用粘贴 Key");
    expect(msg).not.toContain("网页登录开放后");
  });
  it("绑定步骤仍是 Discord 里的 /register", () => {
    expect(msg).toContain("/register");
  });
});
