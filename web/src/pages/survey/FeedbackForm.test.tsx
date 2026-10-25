import { render } from "preact";
import { act } from "preact/test-utils";
import { afterEach, describe, expect, it, vi } from "vitest";
import { FeedbackForm } from "./FeedbackForm";
import { postMember } from "./post";

vi.mock("./post", () => ({ postMember: vi.fn() }));

let host: HTMLElement | null = null;
afterEach(() => {
  if (host) render(null, host);
  host?.remove();
  host = null;
  vi.resetAllMocks();
});

function mount() {
  host = document.createElement("div");
  render(<FeedbackForm />, host);
  const root = host;
  const query = <T extends Element>(selector: string): T => {
    const element = root.querySelector<T>(selector);
    if (!element) throw new Error(`Missing form control ${selector}`);
    return element;
  };
  const text = query<HTMLTextAreaElement>('textarea[name="text"]');
  text.value = "生成时遇到问题";
  act(() => {
    text.dispatchEvent(new Event("input", { bubbles: true }));
  });
  const client = query<HTMLInputElement>('input[name="client"]');
  client.value = "柏宝绘";
  act(() => {
    client.dispatchEvent(new Event("input", { bubbles: true }));
  });
  const version = query<HTMLInputElement>('input[name="version"]');
  version.value = "1.2.3";
  act(() => {
    version.dispatchEvent(new Event("input", { bubbles: true }));
  });
  const requestId = query<HTMLInputElement>('input[name="request_id"]');
  requestId.value = "1a2b3c4d";
  act(() => {
    requestId.dispatchEvent(new Event("input", { bubbles: true }));
  });
  return query<HTMLFormElement>("form");
}

describe("member feedback form", () => {
  it("submits optional client/version/request metadata and shows success", async () => {
    vi.mocked(postMember).mockResolvedValue();
    const form = mount();
    await act(async () => {
      form.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
      await new Promise((resolve) => setTimeout(resolve, 0));
    });
    expect(postMember).toHaveBeenCalledWith("/member/feedback", {
      text: "生成时遇到问题",
      client: "柏宝绘",
      version: "1.2.3",
      request_id: "1a2b3c4d",
    });
    expect(host?.textContent).toContain("反馈已提交");
  });

  it("shows server validation errors", async () => {
    vi.mocked(postMember).mockRejectedValue(new Error("请求编号无效，或不属于你的账号。"));
    const form = mount();
    await act(async () => {
      form.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
      await new Promise((resolve) => setTimeout(resolve, 0));
    });
    expect(host?.textContent).toContain("请求编号无效，或不属于你的账号。");
  });
});
