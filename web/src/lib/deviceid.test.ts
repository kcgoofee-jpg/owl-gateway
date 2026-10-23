import { describe, expect, it } from "vitest";
import { extras, fingerprint } from "./deviceid";

describe("设备指纹补充项", () => {
  it("取不到的项是空串，不抛错；补充项不进指纹分项", async () => {
    const x = await extras();
    for (const k of ["voices", "fonts", "gamut", "hdr", "dpr", "storage"]) expect(k in x).toBe(true);
    expect(x.hdr === "high" || x.hdr === "standard").toBe(true);
    const parts = await fingerprint();
    for (const k of ["voices", "fonts", "gamut", "hdr", "dpr", "storage", "uaModel"])
      expect(k in parts).toBe(false);
  });
});
