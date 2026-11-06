/**
 * 成员页的筛选横栏：一排并列标签，单行、窄屏横向滚动。
 * 选中的标签高亮，再点一次取消（回到「全部成员」）。
 */
import { cx } from "../../../../ui";

export type Chip = {
  id: string;
  label: string;
  count: number;
  hint?: string;
  /** 有人需要处理时用醒目的颜色显示人数 */
  hot?: boolean;
};

export function FilterBar(props: {
  chips: Chip[];
  /** 当前选中的筛选 id；不在 chips 里时，没有标签高亮 */
  active: string;
  onToggle: (id: string) => void;
}) {
  return (
    <div class="mb-filter">
      {/* biome-ignore lint/a11y/useSemanticElements: 一排并列的切换按钮，role=group + aria-label 即标准写法 */}
      <div class="mb-bar" role="group" aria-label="筛选成员">
        <div class="mb-bar__scroll">
          {props.chips.map((c) => (
            <button
              key={c.id}
              type="button"
              class="mb-chip"
              aria-pressed={c.id === props.active}
              title={c.hint}
              onClick={() => props.onToggle(c.id)}
            >
              {c.label}
              <span class={cx("mb-n", c.hot && c.count > 0 && "is-hot")}>{c.count}</span>
            </button>
          ))}
        </div>
      </div>
    </div>
  );
}
