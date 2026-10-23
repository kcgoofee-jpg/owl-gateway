import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { installErrorReporter, isNetworkFailure } from "./api";

describe("isNetworkFailure", () => {
  it("浏览器各家的网络失败文案", () => {
    for (const m of [
      "Failed to fetch",
      "NetworkError when attempting to fetch resource.",
      "TypeError: Load failed",
      "A network error occurred.",
      "The network connection was lost.",
      "AbortError: The operation was aborted.",
      "请求已取消",
      "FAILED TO FETCH",
    ]) {
      expect(isNetworkFailure(m), m).toBe(true);
    }
  });
  it("别的报错不算", () => {
    for (const m of ["TypeError: x is undefined", "ReferenceError: y", "加载失败", ""]) {
      expect(isNetworkFailure(m), m).toBe(false);
    }
  });
});

describe("installErrorReporter", () => {
  const sent: Array<Record<string, unknown>> = [];

  beforeAll(() => {
    vi.stubGlobal("navigator", {
      sendBeacon: (_url: string, blob: Blob) => {
        // happy-dom 的 Blob 没有同步读取：用 text() 取回上报的 JSON
        void blob.text().then((t) => sent.push(JSON.parse(t)));
        return true;
      },
    });
    installErrorReporter("landing");
  });
  beforeEach(() => {
    sent.length = 0;
  });

  const flush = () => new Promise((r) => setTimeout(r, 0));

  it("脚本错误带上 filename（服务端据此丢弃不在 /_owl/ 下的报错）", async () => {
    window.dispatchEvent(
      new ErrorEvent("error", {
        message: "Uncaught TypeError: x is undefined",
        filename: "https://gate.example.top/_owl/assets/app-abc.js",
        lineno: 7,
      }),
    );
    await flush();
    expect(sent).toHaveLength(1);
    expect(sent[0]).toMatchObject({
      page: "landing",
      msg: "Uncaught TypeError: x is undefined",
      src: "https://gate.example.top/_owl/assets/app-abc.js",
      filename: "https://gate.example.top/_owl/assets/app-abc.js",
      line: 7,
    });
  });

  it("Promise 拒绝没有 filename 字段", async () => {
    window.dispatchEvent(
      Object.assign(new Event("unhandledrejection"), { reason: new TypeError("y is not a function") }),
    );
    await flush();
    expect(sent).toHaveLength(1);
    expect(sent[0]?.msg).toBe("y is not a function");
    expect("filename" in (sent[0] ?? {})).toBe(false);
  });

  it("网络失败不上报，也不占用上报名额", async () => {
    window.dispatchEvent(
      Object.assign(new Event("unhandledrejection"), { reason: new TypeError("Failed to fetch") }),
    );
    window.dispatchEvent(
      new ErrorEvent("error", {
        message: "NetworkError when attempting to fetch resource.",
        filename: "/_owl/x.js",
      }),
    );
    await flush();
    expect(sent).toHaveLength(0);
  });
});
