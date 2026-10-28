import { describe, expect, it } from "vitest";
import { claimBody, type RegistrationInfo, validRole } from "./core";

describe("统一领取条件", () => {
  it("完整保留 Discord 字符串 ID，拒绝范围外和非整数", () => {
    const role = { guild: "900000000000000001", role: "900000000000000002", note: "测试身份组" };
    expect(validRole(role)).toBe(true);
    for (const guild of ["0", "01", "1.5", "-1", "18446744073709551616", "text"])
      expect(validRole({ ...role, guild })).toBe(false);
  });
  it("只提交领取条件，不夹带默认权限和只读统计", () => {
    const registration: RegistrationInfo = {
      configured: true,
      active: 5,
      open: true,
      max_users: 20,
      expires_days: 0,
      roles: [],
    };
    expect(claimBody(registration)).toEqual({ open: true, max_users: 20, expires_days: 0, roles: [] });
  });
});
