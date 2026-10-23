import { render } from "preact";
import { act } from "preact/test-utils";
import { afterEach, describe, expect, it, vi } from "vitest";
import { filterOptions, SEARCH_LIMIT, type SearchOption, SearchSelect } from "./SearchSelect";

const OPTS = [
  { value: "1", label: "BRD", hint: "BRD (@brd0889)" },
  { value: "2", label: "暴富", hint: "暴富 (@bunnykittyrcute)" },
  { value: "3", label: "Alice", hint: "Alice (@alice01)" },
];

let host: HTMLElement | null = null;
function mount(value = "", options: SearchOption[] = OPTS) {
  host = document.createElement("div");
  document.body.append(host);
  const onChange = vi.fn();
  render(
    <SearchSelect label="成员" value={value} onChange={onChange} options={options} allLabel="全部成员" />,
    host,
  );
  return { onChange, host };
}
afterEach(() => {
  if (host) render(null, host);
  host?.remove();
  host = null;
});

const input = (h: HTMLElement) => h.querySelector("input") as HTMLInputElement;
const type = async (h: HTMLElement, text: string) => {
  const el = input(h);
  await act(() => {
    el.dispatchEvent(new Event("focus"));
  });
  await act(() => {
    el.value = text;
    el.dispatchEvent(new Event("input", { bubbles: true }));
  });
};
const key = async (h: HTMLElement, k: string) => {
  await act(() => {
    input(h).dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true }));
  });
};
const rows = (h: HTMLElement) => [...h.querySelectorAll("[role=option]")].map((e) => e.textContent);

describe("filterOptions", () => {
  it("名字和备注都能搜，不分大小写，多个词都要命中", () => {
    expect(filterOptions(OPTS, "").length).toBe(3);
    expect(filterOptions(OPTS, "brd").map((o) => o.value)).toEqual(["1"]);
    expect(filterOptions(OPTS, "@BUNNY").map((o) => o.value)).toEqual(["2"]);
    expect(filterOptions(OPTS, "暴富 bunny").map((o) => o.value)).toEqual(["2"]);
    expect(filterOptions(OPTS, "暴富 alice")).toEqual([]);
  });
});

describe("SearchSelect", () => {
  it("没选时输入框是空的、占位写「全部成员」；选了之后显示名字", () => {
    const { host: h } = mount("");
    expect(input(h).value).toBe("");
    expect(input(h).placeholder).toBe("全部成员");
    render(null, h);
    const { host: h2 } = mount("2");
    expect(input(h2).value).toBe("暴富");
  });

  it("点开先是「全部」加全部成员；打字后按名字和 @用户名 过滤，「全部」始终在第一项", async () => {
    const { host: h } = mount();
    await type(h, "");
    expect(rows(h)).toEqual([
      "全部成员",
      "BRDBRD (@brd0889)",
      "暴富暴富 (@bunnykittyrcute)",
      "AliceAlice (@alice01)",
    ]);
    await type(h, "bunny");
    expect(rows(h)).toEqual(["全部成员", "暴富暴富 (@bunnykittyrcute)"]);
    await type(h, "没有这个人");
    expect(rows(h)).toEqual(["全部成员"]);
    expect(h.textContent).toContain("没有匹配「没有这个人」的项");
  });

  it("键盘：↓ 移到下一项，回车选中；Esc 关闭不选", async () => {
    const { host: h, onChange } = mount();
    await type(h, "");
    await key(h, "ArrowDown");
    await key(h, "Enter");
    expect(onChange).toHaveBeenCalledWith("1");
    const { host: h2, onChange: c2 } = mount();
    await type(h2, "");
    await key(h2, "Escape");
    expect(h2.querySelector("[role=listbox]")).toBeNull();
    expect(c2).not.toHaveBeenCalled();
  });

  it("鼠标点一项选中；点「全部」得到空值", async () => {
    const { host: h, onChange } = mount("2");
    await type(h, "");
    await act(() => {
      (h.querySelectorAll("[role=option]")[3] as HTMLElement).dispatchEvent(
        new MouseEvent("mousedown", { bubbles: true, cancelable: true }),
      );
    });
    expect(onChange).toHaveBeenLastCalledWith("3");
    await type(h, "");
    await act(() => {
      (h.querySelectorAll("[role=option]")[0] as HTMLElement).dispatchEvent(
        new MouseEvent("mousedown", { bubbles: true, cancelable: true }),
      );
    });
    expect(onChange).toHaveBeenLastCalledWith("");
  });

  it("项太多时最多列 50 项，并提示继续输入", async () => {
    const many = Array.from({ length: 120 }, (_, i) => ({ value: String(i), label: `成员${i}` }));
    const { host: h } = mount("", many);
    await type(h, "");
    expect(h.querySelectorAll("[role=option]").length).toBe(SEARCH_LIMIT + 1);
    expect(h.textContent).toContain("还有 70 项，继续输入缩小范围");
    await type(h, "成员11");
    expect(h.querySelectorAll("[role=option]").length).toBe(1 + 11); // 成员11、成员110～119
  });

  it("读屏属性：combobox + listbox + option，当前项 aria-activedescendant", async () => {
    const { host: h } = mount();
    await type(h, "");
    expect(input(h).getAttribute("role")).toBe("combobox");
    expect(input(h).getAttribute("aria-expanded")).toBe("true");
    const id = input(h).getAttribute("aria-activedescendant");
    expect(id).toBeTruthy();
    expect(h.querySelector(`#${CSS.escape(id as string)}`)?.getAttribute("role")).toBe("option");
  });
});
