import { render } from "preact";
import { afterEach, describe, expect, it } from "vitest";
import { RequestRow } from "./Requests";
import type { MyRequest } from "./types";

const host = document.createElement("div");
const base: MyRequest = {
  id: 1,
  rid: "abc12345",
  ts: 1,
  kind: "image",
  model: "nai-diffusion-4-5-full",
  status: "ok",
  images: 1,
  anlas: 0,
  site_paid: false,
  wait_s: null,
  dur_s: 1,
  reason: "",
  code: null,
  message: "832x1216/28step 免费",
};

afterEach(() => render(null, host));

describe("request delivery status", () => {
  it("shows generated separately from an interrupted transport", () => {
    render(<RequestRow r={{ ...base, delivery_state: "transport_interrupted" }} />, host);
    expect(host.textContent).toContain("已生成");
    expect(host.textContent).toContain("传输中断");
    expect(host.querySelector(".badge--ok")?.textContent).toBe("已生成");
    expect(host.querySelector(".badge--danger")?.textContent).toBe("传输中断");
  });

  it("keeps an in-flight transport visible", () => {
    render(<RequestRow r={{ ...base, delivery_state: "pending" }} />, host);
    expect(host.textContent).toContain("传输中");
  });

  /**
   * 站长 10-27：传输完成不再提示——图到没到成员自己看得到，服务端也确认不了客户端收到
   * （那需要客户端回执）；老记录没有交付状态，同样不提示。
   */
  it("shows no delivery badge for transport_complete or legacy rows", () => {
    render(<RequestRow r={{ ...base, delivery_state: "transport_complete" }} />, host);
    expect(host.textContent).toContain("已生成");
    expect(host.textContent).not.toContain("服务端传输完成");
    expect(host.textContent).not.toContain("未确认客户端收到");
    expect(host.textContent).not.toContain("传输状态未知");
    expect(host.textContent).not.toContain("传输");
    render(<RequestRow r={base} />, host);
    expect(host.textContent).not.toContain("传输状态未知");
    expect(host.textContent).not.toContain("服务端传输完成");
  });

  it("shows unknown remainder without hiding a partial generated success", () => {
    render(
      <RequestRow r={{ ...base, settlement_status: "uncertain", remaining_anlas: 2, remaining_v5: 1 }} />,
      host,
    );
    expect(host.querySelector(".badge--ok")?.textContent).toBe("已生成");
    expect(host.textContent).toContain("执行结果待核对 · 有未知余量");
    expect(host.textContent).toContain("仍待核对余量：2 Anlas · V5 1");
  });

  it("does not label an unknown execution as a definite failure", () => {
    render(<RequestRow r={{ ...base, status: "error", settlement_status: "uncertain" }} />, host);
    expect(host.textContent).toContain("执行结果待核对");
    expect(host.textContent).not.toContain("失败");
  });

  it("merges the secondary facts into one muted meta line（10-27 排版精简）", () => {
    render(<RequestRow r={{ ...base, correlation_id: "aB12z9", wait_s: 54, dur_s: 37 }} />, host);
    const meta = host.querySelector(".lp-req__meta.muted");
    expect(meta?.textContent).toBe("排队 54 秒 · 出图 37 秒 · 编号 abc12345 · NovelAI 排障号 aB12z9");
    expect(host.querySelector(".lp-req__head")?.textContent).not.toContain("编号");
  });
});
