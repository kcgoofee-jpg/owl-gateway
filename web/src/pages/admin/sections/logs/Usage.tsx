/** 请求日志：每次请求一行；按成员 / 功能 / 状态 / 原因 / 上游错误分类 / 编号筛选，服务端分页。 */
import { ChevronDown } from "lucide-preact";

import { useEffect, useMemo, useRef, useState } from "preact/hooks";
import { fullTime, num, withUnit } from "../../../../lib/format";
import { Badge, Button, CopyField, cx, SearchSelect, Skeleton } from "../../../../ui";
import { useAdmin } from "../../data";
import { LoadError, TableWrap } from "../_kit";
import { asFeatures, type Feature, logFeatures } from "../features";
import { FreshBar, useFrozenList, useScrolledPast } from "../live";
import { memberHref, memberKeyOptions, memberLabel } from "../members/logic";
import { Empty, MemberIdent, Pager, useDebounced, usePageData } from "../members/shared";
import type { Member } from "../members/types";
import {
  deliveryLabel,
  issueExplain,
  kindLabel,
  STATUS_TEXT,
  shortModel,
  statusHeadline,
  statusTone,
  timing,
} from "./format";
import { intParam, useQueryState } from "./query";
import type { CountChip, LogSummary, LogsResp, UsageLog } from "./types";
import { uaLabel } from "./ua";

const COLS = 8;
type Filter = (patch: Record<string, string | null>) => void;

function ChipBar(props: {
  label: string;
  items: Array<{ id: string; label: string; count: number; tone?: "warn" | "danger" }>;
  active: string;
  onToggle: (id: string) => void;
}) {
  if (!props.items.length) return null;
  return (
    // biome-ignore lint/a11y/useSemanticElements: 一排并列的切换按钮，role=group + aria-label 即标准写法
    <div class="lg-chips" role="group" aria-label={props.label}>
      {props.items.map((c) => (
        <button
          key={c.id}
          type="button"
          class={cx("lg-chip", c.tone && `is-${c.tone}`)}
          aria-pressed={c.id === props.active}
          onClick={() => props.onToggle(c.id)}
        >
          {c.label}
          <span class="lg-n">{num(c.count)}</span>
        </button>
      ))}
    </div>
  );
}

function Status(props: { l: UsageLog; onFilter: Filter }) {
  const { l } = props;
  const delivery = l.status === "ok" ? deliveryLabel(l.delivery_state) : null;
  const headline = statusHeadline(l);
  const hint =
    l.status === "error" && l.issue_hint && l.issue_hint !== `上游 ${l.up_status}` ? l.issue_hint : "";
  const onClick = () => {
    if (l.status === "rejected" && l.reason) {
      props.onFilter({ status: "rejected", reason: l.reason, error: null, page: null });
    } else if (l.status === "error" && l.issue) {
      props.onFilter({ status: "error", error: l.issue, reason: null, page: null });
    }
  };
  const clickable = (l.status === "rejected" && !!l.reason) || (l.status === "error" && !!l.issue);
  return (
    <span class="lg-status">
      {clickable ? (
        <button
          type="button"
          class={cx("lg-chip", `is-${statusTone(l.status)}`)}
          title="只看这类"
          onClick={onClick}
        >
          {headline}
        </button>
      ) : (
        <Badge tone={statusTone(l.status)}>{headline}</Badge>
      )}
      {hint && (
        <span class="small muted lg-issue-hint" title={hint}>
          {hint}
        </span>
      )}
      {delivery && (
        <Badge
          tone={delivery.tone}
          title="只表示网关没有完整交付响应，不能据此判断由浏览器、插件或网络哪一端断开。"
        >
          {delivery.text}
        </Badge>
      )}
    </span>
  );
}

function Timing(props: { l: UsageLog }) {
  const t = timing(props.l.wait_ms, props.l.dur_ms);
  if (!t.gen && !t.wait) return <span class="muted">—</span>;
  const total = (props.l.wait_ms ?? 0) + (props.l.dur_ms ?? 0);
  return (
    <span class="nowrap tabular" title={`生成 ${t.gen || "—"} · 排队 ${t.wait || "—"}`}>
      {total >= 1000 ? `${(total / 1000).toFixed(1)} 秒` : `${total} 毫秒`}
    </span>
  );
}

function Cost(props: { l: UsageLog }) {
  const { l } = props;
  const parts = [
    l.images ? <span class="nowrap">{withUnit(l.images, "张")}</span> : null,
    l.anlas ? <span class="nowrap">{+Number(l.anlas).toFixed(1)} A</span> : null,
    l.unconfirmed_anlas > 0 ? (
      <span class="nowrap" title="失败请求可能产生的费用，尚未确认">
        ≈ {l.unconfirmed_anlas} A
      </span>
    ) : null,
    l.tokens ? <span class="nowrap">{num(l.tokens)} tok</span> : null,
  ].filter(Boolean);
  if (!parts.length) return <span class="muted">—</span>;
  return <span class="lg-cost">{parts}</span>;
}

function Client(props: { l: UsageLog }) {
  const ua = props.l.client;
  return (
    <span class={cx("lg-ua", !ua && "muted")} title={ua || undefined}>
      {uaLabel(ua)}
    </span>
  );
}

function Who(props: { l: UsageLog; memberByKey: Map<number, Member> }) {
  const { l } = props;
  if (l.key_id == null) return <span class="muted">{l.key_name || "—"}</span>;
  const m = props.memberByKey.get(l.key_id as number);
  const title = m ? memberLabel(m) : l.key_name || `#${l.key_id}`;
  const ident = m ? (
    <MemberIdent title={title} username={m.discord?.username} avatarUrl={m.discord?.avatar_url} />
  ) : (
    <span class="lg-who-plain">{title}</span>
  );
  if (!m) return ident;
  return (
    <a class="lg-who" href={memberHref(m.id)} title={`打开 ${title}`}>
      {ident}
    </a>
  );
}

function What(props: { l: UsageLog; features: Feature[]; kinds: Record<string, string> }) {
  const { l } = props;
  return (
    <span class="nowrap" title={l.model || undefined}>
      {kindLabel(l.kind, props.features, props.kinds)}
      {l.model ? ` · ${shortModel(l.model)}` : ""}
    </span>
  );
}

function More(props: {
  l: UsageLog;
  memberByKey: Map<number, Member>;
  memberOf: (id: number) => string;
  onFilter: Filter;
}) {
  const { l } = props;
  const m = l.key_id != null ? props.memberByKey.get(l.key_id as number) : undefined;
  const err = l.status === "error";
  const rej = l.status === "rejected";
  return (
    <div class="lg-morebox">
      {(err || rej) && (
        <div class="lg-explain">
          <p class="lg-explain__t">{statusHeadline(l)}</p>
          <p class="small">{err ? issueExplain(l.issue) : l.detail || ""}</p>
          {err && l.issue_hint && l.issue_hint !== `上游 ${l.up_status}` && (
            <p class="small">上游说：{l.issue_hint}</p>
          )}
        </div>
      )}
      <dl class="lg-kv">
        <dt>编号</dt>
        <dd>{l.rid ? <CopyField value={l.rid} label="编号" /> : <span class="muted">—</span>}</dd>
        {l.token_id ? (
          <>
            <dt>上游账号</dt>
            <dd class="mono">{l.token_id}</dd>
          </>
        ) : null}
        <dt>来源 IP</dt>
        <dd class="mono">{l.src || <span class="muted">—</span>}</dd>
        <dt>UA 原文</dt>
        <dd class="lg-wrap">{l.client || <span class="muted">—</span>}</dd>
        {err ? (
          <>
            <dt>原始说明</dt>
            <dd class="lg-wrap">{l.detail || <span class="muted">—</span>}</dd>
          </>
        ) : rej ? null : (
          <>
            <dt>说明</dt>
            <dd class="lg-wrap">{l.detail || <span class="muted">—</span>}</dd>
          </>
        )}
      </dl>
      <div class="lg-more-nav">
        {m && (
          <a class="lg-link" href={memberHref(m.id)}>
            打开成员
          </a>
        )}
        {l.key_id != null && (
          <button
            type="button"
            class="lg-link"
            onClick={() => props.onFilter({ key: props.memberOf(l.key_id as number), page: null })}
          >
            只看此成员的请求
          </button>
        )}
      </div>
    </div>
  );
}

function Toggle(props: { open: boolean; onClick: () => void; id: string }) {
  return (
    <Button
      variant="ghost"
      icon
      size="sm"
      class={cx("lg-expander", props.open && "is-open")}
      aria-expanded={props.open}
      aria-controls={props.id}
      aria-label={props.open ? "收起" : "展开"}
      onClick={props.onClick}
    >
      <ChevronDown size={16} aria-hidden="true" />
    </Button>
  );
}

const STATUS_CHIPS: Array<{ id: keyof LogSummary | ""; label: string; tone?: "warn" | "danger" }> = [
  { id: "", label: "全部" },
  { id: "ok", label: "成功" },
  { id: "rejected", label: "拒绝", tone: "warn" },
  { id: "error", label: "错误", tone: "danger" },
  { id: "cancelled", label: "已取消", tone: "warn" },
];

export function Usage() {
  const { query, set } = useQueryState();
  const members = useAdmin<{ members: Member[] }>("/admin/api/members");
  const catalog = useAdmin<{
    features: Array<{ id: string; label: string }>;
    kinds?: Record<string, string>;
  }>("/admin/api/features");
  const features = useMemo(() => asFeatures(catalog.data?.features ?? []), [catalog.data]);
  const kinds = catalog.data?.kinds ?? {};
  const featureOptions = useMemo(() => logFeatures(features), [features]);
  const [open, setOpen] = useState<Set<number>>(new Set());

  const page = intParam(query.page, 1);
  const per = 100;
  const key = /^\d+(,\d+)*$/.test(query.key ?? "") ? (query.key as string) : "";
  const requestedFeature =
    query.feature === "image" && (query.model_version === "v45" || query.model_version === "v5")
      ? `image_${query.model_version}`
      : query.feature;
  const selectedFeature = featureOptions.some((f) => f.id === requestedFeature)
    ? (requestedFeature as string)
    : "";
  const feature = selectedFeature.startsWith("image_") ? "image" : selectedFeature;
  const modelVersion = selectedFeature === "image_v45" ? "v45" : selectedFeature === "image_v5" ? "v5" : "";
  const error = /^up_[a-z0-9_]{1,16}$/.test(query.error ?? "") ? (query.error as string) : "";
  const status = Object.keys(STATUS_TEXT).includes(query.status ?? "")
    ? (query.status as string)
    : error
      ? "error"
      : "";
  const reason =
    status === "rejected" && /^[a-z0-9_]{1,24}$/.test(query.reason ?? "") ? (query.reason as string) : "";
  const rid = (query.rid ?? "")
    .toLowerCase()
    .replace(/[^0-9a-f]/g, "")
    .slice(0, 16);

  const [ridInput, setRidInput] = useState(rid);
  useEffect(() => setRidInput(rid), [rid]);
  const dRid = useDebounced(ridInput, 350);
  useEffect(() => {
    const clean = dRid
      .trim()
      .toLowerCase()
      .replace(/[^0-9a-f]/g, "")
      .slice(0, 16);
    if (clean !== rid) set({ rid: clean, page: null });
  }, [dRid]);

  const memberList = members.data?.members ?? [];
  // 只在成员数据变化时重建：419 位成员 × 每把 Key，每帧都算会明显掉帧
  const { memberByKey, options } = useMemo(() => {
    const byKey = new Map<number, Member>();
    for (const m of memberList) for (const k of m.all_key_ids ?? []) byKey.set(k, m);
    return { memberByKey: byKey, options: memberKeyOptions(memberList) };
  }, [members.data]);
  // 手输 / 旧链接里的 Key 不在成员列表里时补一项（不就地改记忆化的数组）
  const allOptions =
    key && !options.some((o) => o.value === key)
      ? [{ value: key, label: `Key ${key}`, hint: "" }, ...options]
      : options;
  const memberOf = (id: number) =>
    allOptions.find((o) => o.value.split(",").includes(String(id)))?.value ?? String(id);

  const qs = new URLSearchParams();
  if (key) qs.set("key_id", key);
  if (rid) qs.set("rid", rid);
  if (feature) qs.set("feature", feature);
  if (modelVersion) qs.set("model_version", modelVersion);
  if (status) qs.set("status", status);
  if (reason) qs.set("reason", reason);
  if (error) qs.set("error", error);
  qs.set("per_page", String(per));
  qs.set("page", String(page));
  const { data, error: loadError, pending } = usePageData<LogsResp>(`/admin/api/logs?${qs}`);

  const filters = (
    <div class="xa-toolbar lg-filters-in">
      <div class="field-inline">
        <span class="field-inline__k">成员</span>
        <SearchSelect
          label="成员"
          value={key}
          allLabel="全部成员"
          options={allOptions}
          onChange={(v) => set({ key: v, page: null })}
        />
      </div>
      <label class="field-inline">
        <span class="field-inline__k">功能</span>
        <select
          class="select"
          value={selectedFeature}
          onChange={(e) => {
            const value = e.currentTarget.value;
            const version = value === "image_v45" ? "v45" : value === "image_v5" ? "v5" : null;
            set({ feature: version ? "image" : value, model_version: version, page: null });
          }}
        >
          <option value="">全部</option>
          {featureOptions.map((f) => (
            <option key={f.id} value={f.id}>
              {f.short}
            </option>
          ))}
        </select>
      </label>
      <label class="field-inline">
        <span class="field-inline__k">编号</span>
        <input
          class="input mono lg-ridin"
          type="search"
          aria-label="按请求编号筛选日志"
          placeholder="编号前 16 位"
          value={ridInput}
          onInput={(e) => setRidInput(e.currentTarget.value)}
        />
      </label>
    </div>
  );

  const setRowOpen = (id: number, to: boolean) =>
    setOpen((s) => {
      if (s.has(id) === to) return s;
      const n = new Set(s);
      if (to) n.add(id);
      else n.delete(id);
      return n;
    });
  const toggle = (id: number) => setRowOpen(id, !open.has(id));

  const top = useRef<HTMLDivElement>(null);
  const scrolled = useScrolledPast(top);
  const live = useFrozenList(data?.logs, (l) => l.id, {
    frozen: scrolled || open.size > 0 || page > 1,
    key: qs.toString(),
    pending,
  });
  const rows = live.rows ?? [];
  const summary = data?.summary;
  // 只看已选结果下的细分（拒绝原因 / 上游错误），没选就不占地方
  const sub: CountChip[] =
    status === "rejected"
      ? (data?.reason_counts ?? [])
      : status === "error"
        ? (data?.error_counts ?? [])
        : [];
  const subActive = status === "rejected" ? reason : error;

  return (
    <div class="stack">
      <div class="lg-bar">{filters}</div>

      {loadError && !data ? (
        <LoadError error={loadError} />
      ) : !data ? (
        <Skeleton h="240px" />
      ) : (
        <>
          {summary && (
            <ChipBar
              label="按结果"
              active={status}
              onToggle={(id) =>
                set({
                  status: id && id !== status ? id : null,
                  reason: null,
                  error: null,
                  page: null,
                })
              }
              items={STATUS_CHIPS.map((c) => ({
                id: c.id,
                label: c.label,
                count: c.id ? summary[c.id] : summary.total,
                ...(c.tone && (c.id ? summary[c.id] : 0) > 0 ? { tone: c.tone } : {}),
              }))}
            />
          )}
          {sub.length > 0 && (
            <ChipBar
              label={status === "rejected" ? "拒绝原因" : "上游错误"}
              active={subActive}
              onToggle={(id) =>
                set({
                  [status === "rejected" ? "reason" : "error"]: id && id !== subActive ? id : null,
                  page: null,
                })
              }
              items={sub.map((c) => ({
                id: c.code,
                label: c.label,
                count: c.count,
                tone: status === "rejected" ? ("warn" as const) : ("danger" as const),
              }))}
            />
          )}
          <div ref={top} class={cx("card lg-listcard", pending && "xa-busy")} aria-busy={pending}>
            <FreshBar fresh={live.fresh} changed={live.changed} onApply={live.apply} anchor={top} />
            {rows.length === 0 ? (
              <Empty title="暂无记录" />
            ) : (
              <TableWrap label="请求日志">
                <table class="table lg-table">
                  <caption class="sr-only">请求日志</caption>
                  <thead>
                    <tr>
                      <th scope="col">
                        <span class="sr-only">详情</span>
                      </th>
                      <th scope="col">时间</th>
                      <th scope="col">成员</th>
                      <th scope="col">模型</th>
                      <th scope="col">状态</th>
                      <th scope="col">耗时</th>
                      <th scope="col">消耗</th>
                      <th scope="col">客户端</th>
                    </tr>
                  </thead>
                  <tbody>
                    {rows.flatMap((l) => {
                      const isOpen = open.has(l.id);
                      const id = `lg-d-${l.id}`;
                      const row = (
                        <tr key={l.id} class={cx("lg-row", isOpen && "is-open")}>
                          <td class="lg-exp">
                            <Toggle open={isOpen} id={id} onClick={() => toggle(l.id)} />
                          </td>
                          <td class="nowrap">{fullTime(l.ts)}</td>
                          <td>
                            <Who l={l} memberByKey={memberByKey} />
                          </td>
                          <td>
                            <What l={l} features={features} kinds={kinds} />
                          </td>
                          <td>
                            <Status l={l} onFilter={set} />
                          </td>
                          <td>
                            <Timing l={l} />
                          </td>
                          <td>
                            <Cost l={l} />
                          </td>
                          <td>
                            <Client l={l} />
                          </td>
                        </tr>
                      );
                      return isOpen
                        ? [
                            row,
                            <tr key={`${l.id}-d`} id={id} class="lg-detail-row">
                              <td colSpan={COLS}>
                                <More l={l} memberByKey={memberByKey} memberOf={memberOf} onFilter={set} />
                              </td>
                            </tr>,
                          ]
                        : [row];
                    })}
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
        </>
      )}
    </div>
  );
}
