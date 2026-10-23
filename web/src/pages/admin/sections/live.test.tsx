import { render } from "preact";
import { act } from "preact/test-utils";
import { afterEach, describe, expect, it } from "vitest";
import { edits, useFrozenList } from "./live";

type Row = { id: number; v: string };
type Out = ReturnType<typeof useFrozenList<Row>>;

let host: HTMLElement | null = null;
let out: Out | null = null;

function Probe(props: { rows: Row[] | null; frozen: boolean; k: string; pending: boolean }) {
  out = useFrozenList(props.rows, (r) => r.id, {
    frozen: props.frozen,
    key: props.k,
    pending: props.pending,
  });
  return null;
}

function show(rows: Row[] | null, frozen: boolean, k = "a", pending = false) {
  if (!host) {
    host = document.createElement("div");
    document.body.append(host);
  }
  act(() => render(<Probe rows={rows} frozen={frozen} k={k} pending={pending} />, host as HTMLElement));
  return out as Out;
}

const ids = (o: Out) => o.rows?.map((r) => r.id);

afterEach(() => {
  if (host) render(null, host);
  host?.remove();
  host = null;
  out = null;
});

describe("useFrozenList（看的时候冻结）", () => {
  const first = [
    { id: 3, v: "c" },
    { id: 2, v: "b" },
  ];

  it("没在看：跟着新数据走", () => {
    show(first, false);
    const o = show([{ id: 4, v: "d" }, ...first], false);
    expect(ids(o)).toEqual([4, 3, 2]);
    expect(o.changed).toBe(false);
  });

  it("在看：顺序和条目冻住，数值原地更新，新行只计数；点一下放进来", () => {
    show(first, false);
    let o = show(
      [
        { id: 5, v: "e" },
        { id: 4, v: "d" },
        { id: 3, v: "c2" },
      ],
      true,
    );
    expect(ids(o)).toEqual([3, 2]);
    expect(o.rows?.[0]?.v).toBe("c2");
    expect(o.fresh).toBe(2);
    expect(o.changed).toBe(true);
    act(() => o.apply());
    o = out as Out;
    expect(ids(o)).toEqual([5, 4, 3]);
    expect(o.changed).toBe(false);
  });

  it("只是顺序变了：不算新记录，但提示有变化", () => {
    show(first, false);
    const o = show([...first].reverse(), true);
    expect(ids(o)).toEqual([3, 2]);
    expect(o.fresh).toBe(0);
    expect(o.changed).toBe(true);
  });

  it("换了筛选 / 页码：直接用新数据；请求没回来时不把旧数据记成新条件的", () => {
    show(first, true);
    let o = show(first, true, "b", true);
    expect(ids(o)).toEqual([3, 2]);
    o = show([{ id: 9, v: "z" }], true, "b", false);
    expect(ids(o)).toEqual([9]);
    o = show(
      [
        { id: 10, v: "y" },
        { id: 9, v: "z" },
      ],
      true,
      "b",
    );
    expect(ids(o)).toEqual([9]);
    expect(o.fresh).toBe(1);
  });

  it("自己做了操作：等到操作后的数据，再整份放进来", () => {
    show(first, false);
    show(first, true);
    act(() => {
      edits.value++;
    });
    let o = show(first, true); // 还是操作前那份
    expect(ids(o)).toEqual([3, 2]);
    const after = [{ id: 3, v: "c" }]; // 删掉了 2
    o = show(after, true);
    expect(ids(o)).toEqual([3]);
    // 之后又冻住了：再来新行只计数
    o = show([{ id: 7, v: "n" }, ...after], true);
    expect(ids(o)).toEqual([3]);
    expect(o.fresh).toBe(1);
  });
});
