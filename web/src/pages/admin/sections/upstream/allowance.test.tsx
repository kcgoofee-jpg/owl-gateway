import { render } from "preact";
import { afterEach, describe, expect, it } from "vitest";
import { AllowanceAccountView } from "./allowance";
import type { AllowanceAccount } from "./core";

const account = (over: Partial<AllowanceAccount> = {}): AllowanceAccount => ({
  account: 1,
  percent: null,
  time_until_next_percent_seconds: null,
  current_response_recharge_per_day_estimate: null,
  historical_sample_based_recharge_per_day_estimate: null,
  active: null,
  tier: null,
  expires_at: null,
  is_grace_period: null,
  account_type: null,
  purchased_percent: null,
  fixed_anlas: null,
  purchased_anlas: null,
  is_negative: null,
  checked_at: null,
  low: false,
  uncertain: false,
  error: null,
  stale: false,
  ...over,
});

let host: HTMLElement | null = null;
afterEach(() => {
  if (host) render(null, host);
  host?.remove();
  host = null;
});

describe("upstream allowance status", () => {
  it("shows unknown fields and a stale state without inventing zero balances", () => {
    host = document.createElement("div");
    render(<AllowanceAccountView a={account({ uncertain: true, stale: true })} threshold={20} />, host);
    expect(host.textContent).toContain("状态陈旧");
    expect(host.textContent).toContain("订阅：未知");
    expect(host.textContent).toContain("tier 未知");
    expect(host.textContent).toContain("到期未知");
    expect(host.textContent).toContain("购买 V5 未知");
    expect(host.textContent).toContain("再恢复 1% 需 未知");
    expect(host.textContent).not.toContain("固定 Anlas 0");
  });

  it("shows raw zero as paused while keeping the estimate unknown", () => {
    host = document.createElement("div");
    render(
      <AllowanceAccountView
        a={account({ time_until_next_percent_seconds: 0, current_response_recharge_per_day_estimate: null })}
        threshold={20}
      />,
      host,
    );
    expect(host.textContent).toContain("再恢复 1% 需 0 秒（当前暂停恢复）");
    expect(host.textContent).toContain("当前响应外推估算：未知");
  });
});
