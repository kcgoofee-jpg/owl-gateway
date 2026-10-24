/**
 * 成员页的筛选横栏：一级只留最常用的四颗标签（全部 / 今日活跃 / 被拒 / 需要处理），
 * 低频的收进行尾的「更多」菜单。单行、窄屏横向滚动（滚的是标签那一段，「更多」钉在行尾），没有页签两处。
 * 选中的标签高亮，再点一次取消（回到「全部成员」）；「更多」里选中的在行末回显成一颗可关闭的选中标签。
 * 选中「24 小时被拒过」时，被拒原因另起一行显示在横栏下面（第二层筛选），不插进横栏里。
 */
import { ChevronDown } from "lucide-preact";
import { cx, Menu } from "../../../../ui";

export type Chip = {
  id: string;
  label: string;
  count: number;
  hint?: string;
  /** 有人需要处理时用醒目的颜色显示人数 */
  hot?: boolean;
};
export type Reason = { tag: string; count: number };

/** 收进「更多」菜单的低频筛选（id 与 logic.ts 的 buildFilters 对应） */
const MORE = new Set(["never", "new", "reclaim", "invite"]);

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
  const primary = props.chips.filter((c) => !MORE.has(c.id));
  const more = props.chips.filter((c) => MORE.has(c.id));
  const picked = props.chips.find((c) => c.id === props.active && MORE.has(c.id));

  return (
    <div class="mb-filter">
      {/* biome-ignore lint/a11y/useSemanticElements: 一排并列的切换按钮，role=group + aria-label 即标准写法（和分段按钮一致） */}
      <div class="mb-bar" role="group" aria-label="筛选成员">
        <div class="mb-bar__scroll">
          {primary.map((c) => (
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
        {more.length > 0 && (
          <Menu
            button={(p) => (
              <button type="button" class="mb-chip" {...p}>
                更多
                <ChevronDown size={14} aria-hidden="true" />
              </button>
            )}
            items={more.map((c) => ({
              label: (
                <>
                  {c.label}
                  <span class="mb-n">{c.count}</span>
                </>
              ),
              onSelect: () => props.onToggle(c.id),
            }))}
          />
        )}
        {picked && (
          <button
            type="button"
            class="mb-chip"
            aria-pressed="true"
            title={picked.hint ?? `再点一次取消「${picked.label}」`}
            onClick={() => props.onToggle(picked.id)}
          >
            {picked.label}
            <span class="mb-n">{picked.count}</span>
            <span class="mb-x" aria-hidden="true">
              ×
            </span>
          </button>
        )}
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
