import { render } from "preact";
import { afterEach, describe, expect, it } from "vitest";
import { InformationAccounts, type UpstreamInformationAccount } from "./information";

const row = (over: Partial<UpstreamInformationAccount> = {}): UpstreamInformationAccount => ({
  account: 1,
  ban_status: null,
  ban_message: null,
  email_verified: null,
  account_created_at: null,
  trial_activated: null,
  trial_actions_left: null,
  trial_images_left: null,
  last_success: null,
  last_error: null,
  last_error_at: null,
  stale: true,
  ...over,
});

let host: HTMLElement | null = null;
afterEach(() => {
  if (host) render(null, host);
  host?.remove();
  host = null;
});

describe("upstream account information projection", () => {
  it("shows unknown and stale values without implying a clean account", () => {
    host = document.createElement("div");
    render(<InformationAccounts accounts={[row()]} />, host);
    expect(host.textContent).toContain("封禁状态未知");
    expect(host.textContent).toContain("状态陈旧");
    expect(host.textContent).toContain("邮箱验证：未知");
    expect(host.textContent).toContain("账号创建：未知");
  });

  it("shows ban details and a fixed cached query error", () => {
    host = document.createElement("div");
    render(
      <InformationAccounts
        accounts={[
          row({
            ban_status: "banned",
            ban_message: "synthetic reason",
            email_verified: false,
            last_success: 1700000000,
            last_error: "上游账号信息暂时不可用",
            last_error_at: 1700000100,
            stale: false,
          }),
        ]}
      />,
      host,
    );
    expect(host.textContent).toContain("banned");
    expect(host.textContent).toContain("synthetic reason");
    expect(host.textContent).toContain("上游账号信息暂时不可用");
    expect(host.textContent).toContain("保留最近一次成功读数");
  });
});
