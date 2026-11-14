/** 操作日志：后台 / 系统 / Discord 成员做过的事，服务端分页（每页 30 条）。每行标「手动」或「自动」，可按它筛。 */
import { useMemo, useRef } from "preact/hooks";
import { fullTime } from "../../../../lib/format";
import { Badge, cx, Skeleton } from "../../../../ui";
import { KindTag, LoadError, TableWrap } from "../_kit";
import { FreshBar, useFrozenList, useScrolledPast } from "../live";
import { Empty, Pager, usePageData } from "../members/shared";
import { ACTOR_FILTERS, actionKindGroups, actionLabel, actorFilter } from "./actionNames";
import { intParam, useQueryState } from "./query";
import type { ActionRow, ActionsResp } from "./types";

const Result = (props: { a: ActionRow }) =>
  props.a.ok ? <Badge tone="ok">成功</Badge> : <Badge tone="danger">失败</Badge>;

/**
 * 「发起者」的显示文本：去掉成员自助前缀（类别已由标签表达），旧记录里「后台 1.2.3.4」
 * 这类来源 IP 不再当身份展示（原始值保留在记录里，详情可查）；其余原样显示。
 */
function subjectText(actor: string): string {
  const s = actor.replace(/^(Discord|网页):/, "").trim();
  return s.replace(/^后台\s+\S*([0-9a-fA-F]{0,4}:){2,}\S*|\s*\d{1,3}(\.\d{1,3}){3}$/g, "").trim() || s;
}

/** 「涉及项」：空值显示占位；成员自助操作里对象重复了发起者本人的，不再重复显示。 */
function targetText(a: ActionRow): string {
  const t = (a.target ?? "").trim();
  if (!t || t === a.actor.trim() || (a.kind === "member" && t === subjectText(a.actor))) return "—";
  return t;
}

/** 「手动」/「自动」/「自助」标签 + 操作者；类别认不出的旧记录只显示原文。 */
function Actor(props: { a: ActionRow }) {
  const k = props.a.kind;
  return (
    <span class="lg-actor">
      {(k === "manual" || k === "auto" || k === "member") && <KindTag kind={k} />}
      <span>{subjectText(props.a.actor) || props.a.actor}</span>
    </span>
  );
}

const DETAIL_INLINE = 140;

/** 结构化操作参数以代码显示；长内容折叠，列表里不再铺整段 JSON（原始值一点不少）。 */
function ActionDetail({ value }: { value: string }) {
  const long = value.length > DETAIL_INLINE;
  const code = <code>{value}</code>;
  if (!long) return <>{value}</>;
  return (
    <details class="lg-fold">
      <summary>展开详情（{value.length} 字符）</summary>
      {code}
    </details>
  );
}

export function Actions() {
  const { query, set } = useQueryState();

  const page = intParam(query.page, 1);
  const actor = actorFilter(query.actor);
  const kind = query.action ?? "";
  // 每页固定 100（和请求日志一致，不再提供条数选项）
  const qs = new URLSearchParams({ page: String(page), per_page: "100" });
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
          <span class="field-inline__k">发起者</span>
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
          ) : (
            <TableWrap label="操作日志">
              <table class="table lg-table">
                <caption class="sr-only">操作日志</caption>
                <thead>
                  <tr>
                    <th scope="col">时间</th>
                    <th scope="col">发起者</th>
                    <th scope="col">操作</th>
                    <th scope="col">涉及项</th>
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
                      <td title={a.target}>{targetText(a)}</td>
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
