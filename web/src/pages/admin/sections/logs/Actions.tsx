/** 操作日志：后台 / 系统 / Discord 成员做过的事，服务端分页（每页 30 条）。每行标「手动」或「自动」，可按它筛。 */
import { useMemo, useRef } from "preact/hooks";
import { fullTime } from "../../../../lib/format";
import { Badge, cx, Skeleton } from "../../../../ui";
import { KindTag, LoadError, TableWrap } from "../_kit";
import { FreshBar, useFrozenList, useScrolledPast } from "../live";
import { Empty, NARROW, Pager, useMedia, usePageData } from "../members/shared";
import { ACTOR_FILTERS, actionKindGroups, actionLabel, actorFilter } from "./actionNames";
import { intParam, useQueryState } from "./query";
import type { ActionRow, ActionsResp } from "./types";

const Result = (props: { a: ActionRow }) =>
  props.a.ok ? <Badge tone="ok">成功</Badge> : <Badge tone="danger">失败</Badge>;

/** 「手动」/「自动」标签 + 操作者；成员自己的操作只显示操作者。 */
function Actor(props: { a: ActionRow }) {
  const k = props.a.kind;
  return (
    <span class="lg-actor">
      {(k === "manual" || k === "auto") && <KindTag kind={k} />}
      <span>{props.a.actor}</span>
    </span>
  );
}

/** 结构化操作参数以代码显示，说明与错误仍按普通文字显示。 */
function ActionDetail({ value }: { value: string }) {
  try {
    const parsed: unknown = JSON.parse(value);
    if (parsed !== null && typeof parsed === "object") return <code>{value}</code>;
  } catch {
    /* 普通说明不需要解析。 */
  }
  return <>{value}</>;
}

export function Actions() {
  const { query, set } = useQueryState();
  const narrow = useMedia(NARROW);
  const page = intParam(query.page, 1);
  const actor = actorFilter(query.actor);
  const kind = query.action ?? "";
  const qs = new URLSearchParams({ page: String(page) });
  if (actor) qs.set("actor", actor);
  if (kind) qs.set("action", kind);
  const { data, error, pending } = usePageData<ActionsResp>(`/admin/api/actions?${qs}`);
  const groups = useMemo(() => actionKindGroups(data?.action_kinds ?? []), [data?.action_kinds]);
  // 往下看了、或不在第 1 页：冻住，新记录攒着（新记录总插在最前面，会把正在看的行挤走）
  const top = useRef<HTMLDivElement>(null);
  const scrolled = useScrolledPast(top);
  const live = useFrozenList(data?.actions, (a) => a.id, {
    frozen: scrolled || page > 1,
    key: qs.toString(),
    pending,
  });
  const rows = live.rows ?? [];

  return (
    <div class="stack">
      <div class="xa-toolbar">
        <label class="field-inline">
          <span class="field-inline__k">操作者</span>
          <select
            class="select"
            value={actor}
            onChange={(e) => set({ actor: e.currentTarget.value, page: null })}
          >
            {ACTOR_FILTERS.map((a) => (
              <option key={a.id} value={a.id}>
                {a.label}
              </option>
            ))}
          </select>
        </label>
        <label class="field-inline">
          <span class="field-inline__k">操作类型</span>
          <select
            class="select"
            value={kind}
            onChange={(e) => set({ action: e.currentTarget.value, page: null })}
          >
            <option value="">全部操作</option>
            {groups.map((g) => (
              <optgroup key={g.title} label={g.title}>
                {g.options.map((o) => (
                  <option key={o.value} value={o.value}>
                    {o.label}
                  </option>
                ))}
              </optgroup>
            ))}
          </select>
        </label>
      </div>

      {error && !data ? (
        <LoadError error={error} />
      ) : !data ? (
        <Skeleton h="240px" />
      ) : (
        <div ref={top} class={cx("card lg-listcard", pending && "xa-busy")} aria-busy={pending}>
          <FreshBar fresh={live.fresh} changed={live.changed} onApply={live.apply} anchor={top} />
          {rows.length === 0 ? (
            <Empty title="暂无记录" />
          ) : narrow ? (
            <ul class="lg-cards" aria-label="操作日志">
              {rows.map((a) => (
                <li key={a.id} class="lg-card">
                  <div class="lg-card__top">
                    <span class="small muted">{fullTime(a.ts)}</span>
                    <Result a={a} />
                  </div>
                  <div>
                    <b>{actionLabel(a.action)}</b>
                  </div>
                  <div class="small muted">
                    <Actor a={a} />
                  </div>
                  {a.target && <div class="small">{a.target}</div>}
                  {a.detail && (
                    <div class="small muted">
                      <ActionDetail value={a.detail} />
                    </div>
                  )}
                </li>
              ))}
            </ul>
          ) : (
            <TableWrap label="操作日志">
              <table class="table lg-table">
                <caption class="sr-only">操作日志</caption>
                <thead>
                  <tr>
                    <th scope="col">时间</th>
                    <th scope="col">操作者</th>
                    <th scope="col">操作</th>
                    <th scope="col">对象</th>
                    <th scope="col">详情</th>
                    <th scope="col">结果</th>
                  </tr>
                </thead>
                <tbody>
                  {rows.map((a) => (
                    <tr key={a.id}>
                      <td class="nowrap" title={fullTime(a.ts)}>
                        {fullTime(a.ts)}
                      </td>
                      <td class="nowrap">
                        <Actor a={a} />
                      </td>
                      <td class="nowrap">{actionLabel(a.action)}</td>
                      <td>{a.target}</td>
                      <td class="muted lg-note">
                        <ActionDetail value={a.detail} />
                      </td>
                      <td>
                        <Result a={a} />
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </TableWrap>
          )}
          <Pager
            page={data.page}
            pages={data.pages}
            total={data.total}
            perPage={data.per_page}
            onPage={(p) => set({ page: p === 1 ? null : String(p) })}
          />
        </div>
      )}
    </div>
  );
}
