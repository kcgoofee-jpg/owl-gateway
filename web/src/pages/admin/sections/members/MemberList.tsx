/** 成员列表：宽屏是粘表头的表格（可按列排序），窄屏变成卡片；点一行展开详情。 */
import { ArrowDown, ArrowUp, ChevronDown, ChevronsUpDown } from "lucide-preact";
import { type ComponentChildren, Fragment } from "preact";
import { rel } from "../../../../lib/format";
import { Badge, Button, cx } from "../../../../ui";
import { Avatar, TableWrap } from "../_kit";
import { memberLabel, memberStatus, roleOf, type SortKey, userTags } from "./logic";
import { type DetailActions, MemberDetail } from "./MemberDetail";
import { MiniBar } from "./shared";
import { type MemberRow, ROLE_LABEL } from "./types";

type Props = {
  rows: MemberRow[];
  now: number;
  narrow: boolean;
  sort: SortKey;
  dir: "asc" | "desc";
  onSort: (k: SortKey) => void;
  open: Set<number>;
  onToggle: (id: number) => void;
  focusId: number | null;
  act: DetailActions;
};

function Identity(props: { m: MemberRow }) {
  const { m } = props;
  const title = memberLabel(m);
  const dc = m.discord;
  const handle =
    dc?.username && dc.username !== title ? `@${dc.username}` : m.discord_id ? "" : "后台建的 Key";
  const role = roleOf(m);
  const tag = userTags(m).find((t) => t.tag !== "邀请");
  return (
    <div class="mb-ident">
      <Avatar name={title} fallback={dc?.username} src={dc?.avatar_url} size={32} />
      <div class="mb-ident__t">
        <div class="mb-name">
          <span class="mb-name__t">{title}</span>
          {role !== "regular" && <Badge tone="info">{ROLE_LABEL[role]}</Badge>}
        </div>
        <div class="small muted mb-sub">
          {handle}
          {tag && (
            <span class="xa-chip" title={tag.note || undefined}>
              {tag.tag}
            </span>
          )}
        </div>
      </div>
    </div>
  );
}

function TodayBars(props: { m: MemberRow }) {
  const { m } = props;
  const v5 = m.today.v5 || 0;
  const v45 = Math.max(0, m.today.images - v5);
  return (
    <div class="mb-bars">
      <MiniBar label="V4.5" used={v45} max={m.daily_images > 0 ? m.daily_images : 0} />
      {(m.daily_v5 > 0 || v5 > 0) && <MiniBar label="V5" used={v5} max={m.daily_v5} tone="v5" />}
    </div>
  );
}

function Status(props: { m: MemberRow; now: number }) {
  const s = memberStatus(props.m, props.now);
  return (
    <span class="mb-status">
      {s && (
        <Badge {...(s.tone ? { tone: s.tone } : {})} {...(s.title ? { title: s.title } : {})}>
          {s.text}
        </Badge>
      )}
      {props.m.key_count > 1 && (
        <Badge title="同一成员重置 / 换 Key 的历史已按人合并，记录跨 Key 汇总">
          {props.m.key_count} 把 Key
        </Badge>
      )}
    </span>
  );
}

function Expander(props: { m: MemberRow; open: boolean; onToggle: () => void }) {
  const label = memberLabel(props.m);
  return (
    <Button
      variant="ghost"
      icon
      size="sm"
      class={cx("mb-expander", props.open && "is-open")}
      aria-expanded={props.open}
      aria-controls={`mb-d-${props.m.id}`}
      aria-label={`${props.open ? "收起" : "展开"}${label}的详情`}
      onClick={props.onToggle}
    >
      <ChevronDown size={16} aria-hidden="true" />
    </Button>
  );
}

const interactive = "button, a, select, input, textarea, label, summary, [role=menuitem]";

export function MemberList(props: Props) {
  const { rows, now, sort, dir } = props;
  const sortHead = (key: SortKey, label: ComponentChildren, cls?: string) => {
    const on = sort === key;
    const Icon = on ? (dir === "asc" ? ArrowUp : ArrowDown) : ChevronsUpDown;
    return (
      <th scope="col" class={cls} aria-sort={on ? (dir === "asc" ? "ascending" : "descending") : "none"}>
        <button type="button" class="xa-sort" onClick={() => props.onSort(key)}>
          {label}
          <Icon size={13} aria-hidden="true" />
        </button>
      </th>
    );
  };

  if (props.narrow) {
    return (
      <ul class="mb-cards" aria-label="成员列表">
        {rows.map((m) => {
          const open = props.open.has(m.id);
          return (
            <li key={m.id} id={`mb-m-${m.id}`} class={cx("mb-card", props.focusId === m.id && "is-focus")}>
              <div class="mb-card__top">
                <Identity m={m} />
                <Expander m={m} open={open} onToggle={() => props.onToggle(m.id)} />
              </div>
              <Status m={m} now={now} />
              <TodayBars m={m} />
              <p class="small muted">
                近 7 天 {m.week.images} 张 · 累计 {m.total_images} 张 · 最近使用 {rel(m.last_used_at, now)}
              </p>
              {open && (
                <div id={`mb-d-${m.id}`}>
                  <MemberDetail m={m} now={now} act={props.act} />
                </div>
              )}
            </li>
          );
        })}
      </ul>
    );
  }

  return (
    <TableWrap label="成员列表">
      <table class="table mb-table">
        <caption class="sr-only">成员列表，点一行展开详情</caption>
        <thead>
          <tr>
            {sortHead("name", "成员")}
            {sortHead("today", "今日用量")}
            {sortHead("week", "近 7 天", "num")}
            {sortHead("total", "累计", "num")}
            <th scope="col">状态</th>
            {sortHead("recent", "最近使用")}
            <th scope="col">
              <span class="sr-only">展开</span>
            </th>
          </tr>
        </thead>
        <tbody>
          {rows.map((m) => {
            const open = props.open.has(m.id);
            return (
              <Fragment key={m.id}>
                <tr
                  id={`mb-m-${m.id}`}
                  class={cx("mb-row", open && "is-open", props.focusId === m.id && "is-focus")}
                  onClick={(e) => {
                    if ((e.target as HTMLElement).closest(interactive)) return;
                    if (String(getSelection?.() ?? "").length) return; // 正在选中文字时不切换
                    props.onToggle(m.id);
                  }}
                >
                  <td>
                    <Identity m={m} />
                  </td>
                  <td class="mb-today">
                    <TodayBars m={m} />
                  </td>
                  <td class="num">{m.week.images}</td>
                  <td class="num">{m.total_images}</td>
                  <td>
                    <Status m={m} now={now} />
                  </td>
                  <td class="nowrap" title={m.last_used_at ? undefined : "从未使用"}>
                    {rel(m.last_used_at, now)}
                  </td>
                  <td class="num">
                    <Expander m={m} open={open} onToggle={() => props.onToggle(m.id)} />
                  </td>
                </tr>
                {open && (
                  <tr class="mb-detail-row">
                    <td colSpan={7} id={`mb-d-${m.id}`}>
                      <MemberDetail m={m} now={now} act={props.act} />
                    </td>
                  </tr>
                )}
              </Fragment>
            );
          })}
        </tbody>
      </table>
    </TableWrap>
  );
}
