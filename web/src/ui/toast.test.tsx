import { render } from "preact";
import { afterEach, describe, expect, it } from "vitest";
import { Toaster, toast } from "./index";

const host = document.createElement("div");

afterEach(() => render(null, host));

describe("toasts", () => {
  it("announces a successful save in the polite status region", () => {
    toast("额度已保存并同步", { ms: 1000 });
    render(<Toaster />, host);
    const status = host.querySelector('[role="status"]');
    expect(status?.getAttribute("aria-live")).toBe("polite");
    expect(status?.textContent).toContain("额度已保存并同步");
  });
});
