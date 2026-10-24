import { render } from "preact";
import { act } from "preact/test-utils";
import { afterEach, describe, expect, it, vi } from "vitest";
import { type Chip, FilterBar } from "./FilterBar";

const CHIPS: Chip[] = [
  { id: "all", label: "全部成员", count: 238 },
  { id: "today", label: "今日活跃", count: 87 },
  { id: "rejected", label: "24 小时被拒过", count: 48 },
  { id: "attention", label: "需要处理", count: 2, hot: true },
  { id: "never", label: "还没出过图", count: 12 },
  { id: "invite", label: "邀请 Key", count: 3 },
];
const REASONS = [
  { tag: "请求过快", count: 8 },
  { tag: "角色超 6 个", count: 4 },
];

let host: HTMLElement | null = null;
function mount(p: Partial<Parameters<typeof FilterBar>[0]> = {}) {
  host = document.createElement("div");
  document.body.append(host);
  const props = {
    chips: CHIPS,
    active: "all",
    onToggle: vi.fn(),
    reasons: REASONS,
    reason: null,
    onReason: vi.fn(),
    reasonsOf: "rejected",
    ...p,
  };
  render(<FilterBar {...props} />, host);
  return { props, host };
}
afterEach(() => {
  if (host) render(null, host);
  host?.remove();
  host = null;
});

const buttons = (h: HTMLElement) => [...h.querySelectorAll("button")];
const btn = (h: HTMLElement, text: string) => {
  const b = buttons(h).find((x) => x.textContent?.startsWith(text));
  if (!b) throw new Error(`没有「${text}」`);
  return b;
};
const bar = (h: HTMLElement) => h.querySelector("[aria-label='筛选成员']") as HTMLElement;
const openMore = async (h: HTMLElement) => {
  await act(async () => {
    btn(h, "更多").click();
  });
  return h.querySelector("[role=menu]") as HTMLElement;
};

describe("FilterBar", () => {
  it("一级只留四颗常用标签 + 「更多」，低频筛选不直接摆在横栏里", () => {
    const { host: h } = mount({ active: "today" });
    expect(buttons(bar(h)).map((b) => b.textContent)).toEqual([
      "全部成员238",
      "今日活跃87",
      "24 小时被拒过48",
      "需要处理2",
      "更多",
    ]);
    expect(
      buttons(bar(h))
        .slice(0, 4)
        .map((b) => b.getAttribute("aria-pressed")),
    ).toEqual(["false", "true", "false", "false"]);
    expect(h.querySelector("select")).toBeNull();
    expect(bar(h).getAttribute("aria-label")).toBe("筛选成员");
    expect(buttons(h).some((b) => /还没出过图|邀请 Key/.test(b.textContent ?? ""))).toBe(false);
  });
  it("点标签 = 把它的 id 交给上层（再点取消由上层处理）", () => {
    const { props, host: h } = mount({ active: "today" });
    btn(h, "今日活跃").click();
    expect(props.onToggle).toHaveBeenCalledWith("today");
    btn(h, "24 小时被拒过").click();
    expect(props.onToggle).toHaveBeenLastCalledWith("rejected");
  });
  it("「更多」菜单里是低频筛选，带人数，点选 = 交给上层并收起菜单", async () => {
    const { props, host: h } = mount();
    const menu = await openMore(h);
    expect([...menu.querySelectorAll("[role=menuitem]")].map((b) => b.textContent)).toEqual([
      "还没出过图12",
      "邀请 Key3",
    ]);
    await act(async () => {
      (menu.querySelector("[role=menuitem]") as HTMLElement).click();
    });
    expect(props.onToggle).toHaveBeenCalledWith("never");
    expect(h.querySelector("[role=menu]")).toBeNull();
  });
  it("选中的「更多」筛选在行末回显成可关闭的选中标签，点它 = 取消", () => {
    const { props, host: h } = mount({ active: "never" });
    const pick = btn(h, "还没出过图");
    expect(pick.getAttribute("aria-pressed")).toBe("true");
    expect(pick.textContent).toContain("×");
    expect(buttons(bar(h)).indexOf(pick)).toBe(buttons(bar(h)).length - 1);
    pick.click();
    expect(props.onToggle).toHaveBeenCalledWith("never");
  });
  it("需要处理有人时人数标成醒目色，0 时不标", () => {
    const { host: h } = mount();
    expect(btn(h, "需要处理").querySelector(".mb-n")?.className).toContain("is-hot");
    render(null, h);
    render(
      <FilterBar
        chips={CHIPS.map((c) => (c.hot ? { ...c, count: 0 } : c))}
        active="all"
        onToggle={() => {}}
        reasons={[]}
        reason={null}
        onReason={() => {}}
        reasonsOf="rejected"
      />,
      h,
    );
    expect(btn(h, "需要处理").querySelector(".mb-n")?.className).not.toContain("is-hot");
  });
  it("被拒原因只在「24 小时被拒过」选中时出现，另起一行，不插进横栏", () => {
    const off = mount({ active: "all" });
    expect(off.host.querySelector(".mb-reasons")).toBeNull();
    render(null, off.host);
    off.host.remove();

    const { host: h } = mount({ active: "rejected" });
    const b = bar(h);
    expect(buttons(b).map((x) => x.textContent)).toEqual([
      "全部成员238",
      "今日活跃87",
      "24 小时被拒过48",
      "需要处理2",
      "更多",
    ]);
    const row = h.querySelector("[aria-label='被拒原因']") as HTMLElement;
    expect(row.classList.contains("mb-reasons")).toBe(true);
    expect(b.contains(row)).toBe(false);
    expect(buttons(row).map((x) => x.textContent)).toEqual(["请求过快8", "角色超 6 个4"]);
  });
  it("原因是第二层：点选 / 再点取消", () => {
    const { props, host: h } = mount({ active: "rejected" });
    btn(h, "请求过快").click();
    expect(props.onReason).toHaveBeenCalledWith("请求过快");
    render(null, h);
    render(<FilterBar {...props} reason="请求过快" />, h);
    expect(btn(h, "请求过快").getAttribute("aria-pressed")).toBe("true");
    expect(btn(h, "角色超 6 个").getAttribute("aria-pressed")).toBe("false");
    btn(h, "请求过快").click();
    expect(props.onReason).toHaveBeenLastCalledWith(null);
  });
  it("没有原因可选时不画原因那一行", () => {
    const { host: h } = mount({ active: "rejected", reasons: [] });
    expect(h.querySelector(".mb-reasons")).toBeNull();
  });
  it("切换标签不滚动（不调用 scrollIntoView）", () => {
    const spy = vi.fn();
    const orig = Element.prototype.scrollIntoView;
    Element.prototype.scrollIntoView = spy;
    const { props, host: h } = mount({ active: "all" });
    render(<FilterBar {...props} active="rejected" />, h);
    render(<FilterBar {...props} active="rejected" reason="请求过快" />, h);
    Element.prototype.scrollIntoView = orig;
    expect(spy).not.toHaveBeenCalled();
  });
  it("没有「快被回收」标签（回收关闭）时「更多」里也没有它", async () => {
    const { host: h } = mount();
    expect(buttons(h).some((b) => b.textContent?.includes("快被回收"))).toBe(false);
    const menu = await openMore(h);
    expect(menu.textContent).not.toContain("快被回收");
  });
});
