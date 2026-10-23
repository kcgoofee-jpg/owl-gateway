/**
 * 可搜索的「多选」选择框：用法和 SearchSelect 一样，但能同时选中好几项（例如一次挑出几个疑似小号）。
 * 选中项不在框里堆叠，交给外面的 chips 一行展示；这里只负责「搜索 + 勾选 / 取消」，选中后下拉不关，方便连选。
 * value 是已选项的 value 数组；空数组 = 没筛选（全部）。
 */
import { useMemo } from "preact/hooks";
import { ComboNotes, filterOptions, SEARCH_LIMIT, type SearchOption, useComboList } from "./SearchSelect";

export function SearchMultiSelect(props: {
  values: string[];
  onChange: (values: string[]) => void;
  options: SearchOption[];
  /** 没选时输入框占位，例如「全部成员」 */
  allLabel: string;
  /** 读屏用的名字 */
  label: string;
}) {
  const { values, onChange, options, allLabel } = props;
  const { id, listId, input, root, open, query, setQuery, active, setActive, keyHandler, inputProps } =
    useComboList();

  const picked = new Set(values);
  const matches = useMemo(() => filterOptions(options, query), [options, query]);
  const rows = matches.slice(0, SEARCH_LIMIT);
  const more = matches.length - rows.length;

  const toggle = (v: string) => {
    if (!v) return;
    onChange(picked.has(v) ? values.filter((x) => x !== v) : [...values, v]);
    // 选中后保持展开、清空搜索词，方便连着挑下一个
    setQuery("");
    setActive(0);
    input.current?.focus();
  };

  const onKey = keyHandler(Math.max(rows.length, 1), (e) => {
    if (open && rows[active]) {
      e.preventDefault();
      toggle(rows[active].value);
    }
  });

  return (
    <div class="ss" ref={root}>
      <input
        {...inputProps}
        aria-label={props.label}
        placeholder={values.length ? `已选 ${values.length} 人，继续添加…` : allLabel}
        value={query}
        onKeyDown={onKey}
      />
      {open && (
        <div class="ss__list" id={listId} role="listbox" aria-multiselectable="true" aria-label={props.label}>
          {rows.map((o, i) => (
            <div
              key={o.value}
              id={`${id}-o${i}`}
              role="option"
              tabIndex={-1}
              aria-selected={picked.has(o.value)}
              class={`ss__opt${i === active ? " is-active" : ""}${picked.has(o.value) ? " is-selected" : ""}`}
              onMouseDown={(e) => {
                e.preventDefault();
                toggle(o.value);
              }}
              onMouseEnter={() => setActive(i)}
            >
              <span class="ss__check" aria-hidden="true">
                {picked.has(o.value) ? "✓" : ""}
              </span>
              <span class="ss__label">{o.label}</span>
              {o.hint && <span class="ss__hint">{o.hint}</span>}
            </div>
          ))}
          <ComboNotes empty={rows.length === 0} query={query} more={more} />
        </div>
      )}
    </div>
  );
}
