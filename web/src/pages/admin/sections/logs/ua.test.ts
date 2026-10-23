import { describe, expect, it } from "vitest";
import { uaLabel } from "./ua";

describe("uaLabel", () => {
  it("手机和电脑浏览器", () => {
    expect(
      uaLabel("Mozilla/5.0 (Linux; Android 16; V2338A) AppleWebKit/537.36 Chrome/149.0 Mobile Safari/537.36"),
    ).toBe("Android 16 · Chrome");
    expect(uaLabel("Mozilla/5.0 (Windows NT 10.0; Win64; x64) Chrome/149.0 Safari/537.36 Edg/149.0")).toBe(
      "Windows · Edge",
    );
    expect(
      uaLabel("Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit Version/18.0 Safari/604.1"),
    ).toBe("iOS · Safari");
  });
  it("脚本和 SDK 直接给原文，没记录是「—」", () => {
    expect(uaLabel("python-requests/2.31.0")).toBe("python-requests/2.31.0");
    expect(uaLabel("axios/1.7.2 something-very-long-user-agent-string")).toBe("axios/1.7.2");
    expect(uaLabel("DSH-Tavern-Image/1.2 (Windows NT 10.0)")).toBe("DSH-Tavern-Image/1.2");
    expect(uaLabel("a-really-long-client-name-without-spaces/1.0")).toBe("a-really-long-client-nam…");
    expect(uaLabel("")).toBe("—");
  });
});
