import { render } from "preact";
import { afterEach, describe, expect, it } from "vitest";
import type { MyRequest } from "./types";
import { RequestRow } from "./Requests";

const host = document.createElement("div");
const base: MyRequest = {
  id: 1, rid: "abc12345", ts: 1, kind: "image", model: "nai-diffusion-4-5-full",
  status: "ok", images: 1, anlas: 0, site_paid: false, wait_s: null, dur_s: 1,
  reason: "", code: null, message: "832x1216/28step 免费",
};

afterEach(() => render(null, host));

describe("request delivery status", () => {
  it("shows generated separately from an interrupted transport", () => {
    render(<RequestRow r={{ ...base, delivery_state: "transport_interrupted" }} />, host);
    expect(host.textContent).toContain("已生成");
    expect(host.textContent).toContain("传输中断");
    expect(host.textContent).not.toContain("客户端已收到");
    expect(host.querySelector(".badge--ok")?.textContent).toBe("已生成");
    expect(host.querySelector(".badge--danger")?.textContent).toBe("传输中断");
  });

  it("does not treat legacy rows without a delivery field as received", () => {
    render(<RequestRow r={base} />, host);
    expect(host.textContent).toContain("已生成");
    expect(host.textContent).toContain("传输状态未知");
    expect(host.textContent).not.toContain("服务端传输完成");
  });
});
