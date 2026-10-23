/**
 * 成员页的筛选横栏：一排标签（名称 + 实时人数），单行、窄屏横向滚动，不换行，也没有页签 + 下拉两处。
 * 选中的标签高亮，再点一次取消（回到「全部成员」）。选中「24 小时被拒过」时，被拒原因另起一行
 * 显示在横栏下面（第二层筛选），不插进横栏里——切换时横栏本身不动、不横向跳。
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
export type Reason = { tag: string; count: number };

export function FilterBar(props: {
  chips: Chip[];
  /** 当前选中的筛选 id；不在 chips 里时，没有标签高亮 */
  active: string;
  onToggle: (id: string) => void;
  /** reasonsOf 那个标签被选中时，在横栏下面另起一行显示的原因 */
  reasons: Reason[];
  reason: string | null;
  onReason: (tag: string | null) => void;
  /** 展开原因的那个标签的 id */
  reasonsOf: string;
}) {
  const showReasons = props.active === props.reasonsOf && props.reasons.length > 0;

  return (
    <div class="mb-filter">
      {/* biome-ignore lint/a11y/useSemanticElements: 一排并列的切换按钮，role=group + aria-label 即标准写法（和分段按钮一致） */}
      <div class="mb-bar" role="group" aria-label="筛选成员">
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
      {showReasons && (
        // biome-ignore lint/a11y/useSemanticElements: 同上，一组并列的切换按钮
        <div class="mb-reasons" role="group" aria-label="被拒原因">
          <span class="mb-reasons__lead" aria-hidden="true">
            原因
          </span>
          {props.reasons.map((r) => (
            <button
              key={r.tag}
              type="button"
              class="mb-chip is-sub"
              aria-pressed={props.reason === r.tag}
              title={`近 24 小时因「${r.tag}」被拒的成员`}
              onClick={() => props.onReason(props.reason === r.tag ? null : r.tag)}
            >
              {r.tag}
              <span class="mb-n">{r.count}</span>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
