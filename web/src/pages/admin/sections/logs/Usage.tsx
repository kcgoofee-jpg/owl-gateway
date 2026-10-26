/** 请求日志：每次请求一行（成员、模型、状态、耗时、消耗、来源 IP、客户端）；按成员 / 功能 / 状态 / IP / 编号筛选，服务端分页。 */
import { ChevronDown } from "lucide-preact";
import { Fragment } from "preact";
import { useEffect, useRef, useState } from "preact/hooks";
import { fullTime, maskIp, num, withUnit } from "../../../../lib/format";
import { Badge, Button, CopyField, cx, SearchSelect, Skeleton } from "../../../../ui";
import { useAdmin } from "../../Admin";
import { LoadError, TableWrap } from "../_kit";
import { STATUS_TEXT, statusText, statusTone } from "../audit/auditData";
import { FEATURES } from "../features";
import { FreshBar, useFrozenList, useScrolledPast } from "../live";
import { memberKeyOptions } from "../members/logic";
import { Empty, NARROW, Pager, useDebounced, useMedia, usePageData } from "../members/shared";
import type { Member } from "../members/types";
import { kindLabel, shortModel, timing } from "./format";
import { intParam, useQueryState } from "./query";
import type { LogSummary, LogsResp, UsageLog } from "./types";
import { uaLabel } from "./ua";

const SIZES = [20, 50, 100];
const STATUSES = Object.entries(STATUS_TEXT).map(([id, label]) => ({ id, label }));
const COLS = 9;

/** 完整的 IPv4 / IPv6 才拿去筛（输入到一半不筛） */
export function isIp(s: string): boolean {
  const v4 = /^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})$/.exec(s);
  if (v4) return v4.slice(1).every((x) => Number(x) <= 255);
  return /^[0-9a-f:.]+$/i.test(s) && (s.match(/:/g)?.length ?? 0) >= 2 && s.length <= 45;
}

type Filter = (patch: Record<string, string | null>) => void;

function Status(props: { l: UsageLog; onFilter: Filter }) {
  const { l } = props;
  const label = l.status === "rejected" ? l.reason_label : "";
  return (
    <span class="lg-status">
      <Badge tone={statusTone(l.status)}>{statusText(l.status)}</Badge>
      {l.status === "rejected" &&
        (l.reason ? (
          <button
            type="button"
            class="lg-link small"
            title="只看这个原因"
            onClick={() => props.onFilter({ status: "rejected", reason: l.reason, page: null })}
          >
            {label}
          </button>
        ) : (
          <span class="small muted">{label}</span>
        ))}
      {l.status === "error" && l.up_status > 0 && <span class="small muted">上游 {l.up_status}</span>}
    </span>
  );
}

function Timing(props: { l: UsageLog }) {
  const t = timing(props.l.wait_ms, props.l.dur_ms);
  if (!t.gen && !t.wait) return <span class="muted">—</span>;
  return (
    <span class="lg-timing" title="生成：上游处理时间；等：排队 + 冷却">
      {t.gen && <span class="nowrap">生成 {t.gen}</span>}
      {t.wait && <span class="nowrap muted">等 {t.wait}</span>}
    </span>
  );
}

function Cost(props: { l: UsageLog }) {
  const { l } = props;
  const parts = [
    l.images ? <span class="nowrap">{withUnit(l.images, "张")}</span> : null,
    l.anlas ? <span class="nowrap">{+Number(l.anlas).toFixed(2)} Anlas</span> : null,
    l.unconfirmed_anlas > 0 ? (
      <Badge tone="warn" title="失败请求可能产生的费用，实际扣款尚未确认">
        ≈ {l.unconfirmed_anlas}
      </Badge>
    ) : null,
    l.tokens ? <span class="nowrap">{num(l.tokens)} tokens</span> : null,
  ].filter(Boolean);
  if (!parts.length) return <span class="muted">—</span>;
  return <span class="lg-cost">{parts}</span>;
}

/**
 * 来源 IP：默认只展示打码后的前两段（IPv4）/ 前两组（IPv6），点「显示」才展开完整 IP。
 * 展开时本应记一条审计（谁看了完整 IP），但审计接口还没有，这里先不发请求；等后端加接口再接。
 */
function Ip(props: { l: UsageLog; onFilter: Filter }) {
  const ip = props.l.src;
  const [shown, setShown] = useState(false);
  if (!ip) return <span class="muted">—</span>;
  const masked = maskIp(ip);
  // 库里本来就是打码值（旧数据是 203.0.*.*）：没有再可展开的完整 IP，只显示打码值
  if (masked === ip) return <span class="mono lg-ip">{masked}</span>;
  return (
    <span class="lg-ipis">
      {shown ? (
        <button
          type="button"
          class="lg-link mono lg-ip"
          title={`只看 ${ip}`}
          onClick={() => props.onFilter({ ip, page: null })}
        >
          {ip}
        </button>
      ) : (
        <span class="mono lg-ip">{masked}</span>
      )}
      <button
        type="button"
        class="lg-link lg-iptoggle"
        aria-pressed={shown}
        aria-label={`${shown ? "隐藏" : "显示"}第 ${props.l.id} 条请求的完整 IP`}
        onClick={() => setShown((v) => !v)}
      >
        {shown ? "隐藏" : "显示"}
      </button>
    </span>
  );
}

function Client(props: { l: UsageLog }) {
  const ua = props.l.client;
  return (
    <span class={cx("lg-ua", !ua && "muted")} title={ua || undefined}>
      {uaLabel(ua)}
    </span>
  );
}

function Who(props: { l: UsageLog; memberOf: (id: number) => string; onFilter: Filter }) {
  const { l } = props;
  if (l.key_id == null) return <span class="muted">{l.key_name || "—"}</span>;
  return (
    <button
      type="button"
      class="lg-link lg-who"
      title="只看这位成员"
      onClick={() => props.onFilter({ key: props.memberOf(l.key_id as number), page: null })}
    >
      {l.key_name || `#${l.key_id}`}
    </button>
  );
}

function What(props: { l: UsageLog }) {
  const { l } = props;
  return (
    <span class="nowrap" title={l.model || undefined}>
      {kindLabel(l.kind)}
      {l.model ? ` · ${shortModel(l.model)}` : ""}
    </span>
  );
}

/** 展开后的客户端特征：字段名 → 标签 */
const HINTS = [
  ["xrw", "App 包名"],
  ["lang", "语言"],
  ["chua", "浏览器标识"],
  ["hfp", "请求头指纹"],
  ["bid", "浏览器标记"],
  ["origin", "来源页"],
] as const;

/** 展开后的详情：编号、上游账号、完整 UA、客户端特征、说明 */
function More(props: { l: UsageLog }) {
  const { l } = props;
  return (
    <dl class="lg-kv">
      <dt>编号</dt>
      <dd>{l.rid ? <CopyField value={l.rid} label="编号" /> : <span class="muted">—</span>}</dd>
      {l.correlation_id && (
        <>
          <dt>NovelAI 排障号</dt>
          <dd>
            <CopyField value={l.correlation_id} label="排障号" />
          </dd>
        </>
      )}
      <dt>上游账号</dt>
      <dd class="mono">{l.token_id || <span class="muted">—</span>}</dd>
      <dt>UA 原文</dt>
      <dd class="lg-wrap">{l.client || <span class="muted">—</span>}</dd>
      {HINTS.map(([k, label]) => (
        <Fragment key={k}>
          <dt>{label}</dt>
          <dd class="lg-wrap mono">{l[k] || <span class="muted">—</span>}</dd>
        </Fragment>
      ))}
      <dt>说明</dt>
      <dd class="lg-wrap">{l.detail || <span class="muted">—</span>}</dd>
    </dl>
  );
}

function Summary(props: { s: LogSummary }) {
  const { s } = props;
  return (
    <p class="lg-sum small muted">
      <span>共 {num(s.total)}</span>
      <span>成功 {num(s.ok)}</span>
      <span>拒绝 {num(s.rejected)}</span>
      <span>错误 {num(s.error)}</span>
    </p>
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

export function Usage() {
  const { query, set } = useQueryState();
  const narrow = useMedia(NARROW);
  const members = useAdmin<{ members: Member[] }>("/admin/api/members");
  const [open, setOpen] = useState<Set<number>>(new Set());

  const page = intParam(query.page, 1);
  const per = intParam(query.per, 20, SIZES);
  const key = /^\d+(,\d+)*$/.test(query.key ?? "") ? (query.key as string) : "";
  const feature = FEATURES.some((f) => f.id === query.feature) ? (query.feature as string) : "";
  const status = STATUSES.some((s) => s.id === query.status) ? (query.status as string) : "";
  const reason =
    status === "rejected" && /^[a-z0-9_]{1,24}$/.test(query.reason ?? "") ? (query.reason as string) : "";
  const ip = isIp(query.ip ?? "") ? (query.ip as string) : "";
  const rid = (query.rid ?? "").toLowerCase().replace(/[^0-9a-f]/g, "");

  const [ridInput, setRidInput] = useState(rid);
  useEffect(() => setRidInput(rid), [rid]);
  const dRid = useDebounced(ridInput, 350);
  useEffect(() => {
    const clean = dRid
      .trim()
      .toLowerCase()
      .replace(/[^0-9a-f]/g, "");
    if (clean !== rid) set({ rid: clean, page: null });
  }, [dRid]);

  const [ipInput, setIpInput] = useState(ip);
  useEffect(() => setIpInput(ip), [ip]);
  const dIp = useDebounced(ipInput, 350);
  useEffect(() => {
    const clean = dIp.trim();
    const next = isIp(clean) ? clean : "";
    if (next !== ip) set({ ip: next, page: null });
  }, [dIp]);

  const memberList = members.data?.members ?? [];
  const options = memberKeyOptions(memberList);
  if (key && !options.some((o) => o.value === key))
    options.unshift({ value: key, label: `Key ${key}`, hint: "" });
  const memberOf = (id: number) =>
    options.find((o) => o.value.split(",").includes(String(id)))?.value ?? String(id);

  const qs = new URLSearchParams();
  if (key) qs.set("key_id", key);
  if (rid) qs.set("rid", rid);
  if (feature) qs.set("feature", feature);
  if (status) qs.set("status", status);
  if (reason) qs.set("reason", reason);
  if (ip) qs.set("ip", ip);
  qs.set("per_page", String(per));
  qs.set("page", String(page));
  const { data, error, pending } = usePageData<LogsResp>(`/admin/api/logs?${qs}`);

  // 地址栏里带的原因码不在短说明表里（如 param）：照原码列出来，下拉框才能显示选中
  const known = data?.reasons ?? [];
  const reasons =
    !reason || known.some((r) => r.code === reason) ? known : [...known, { code: reason, label: reason }];

  const filters = (
    <div class="xa-toolbar lg-filters-in">
      <div class="field-inline">
        <span class="field-inline__k">成员</span>
        <SearchSelect
          label="成员"
          value={key}
          allLabel="全部成员"
          options={options}
          onChange={(v) => set({ key: v, page: null })}
        />
      </div>
      <label class="field-inline">
        <span class="field-inline__k">功能</span>
        <select
          class="select"
          value={feature}
          onChange={(e) => set({ feature: e.currentTarget.value, page: null })}
        >
          <option value="">全部</option>
          {FEATURES.map((f) => (
            <option key={f.id} value={f.id}>
              {f.short}
            </option>
          ))}
        </select>
      </label>
      <label class="field-inline">
        <span class="field-inline__k">状态</span>
        <select
          class="select"
          value={status}
          onChange={(e) => set({ status: e.currentTarget.value, reason: null, page: null })}
        >
          <option value="">全部</option>
          {STATUSES.map((s) => (
            <option key={s.id} value={s.id}>
              {s.label}
            </option>
          ))}
        </select>
      </label>
      {status === "rejected" && (
        <label class="field-inline">
          <span class="field-inline__k">原因</span>
          <select
            class="select"
            value={reason}
            onChange={(e) => set({ reason: e.currentTarget.value, page: null })}
          >
            <option value="">全部</option>
            {reasons.map((r) => (
              <option key={r.code} value={r.code}>
                {r.label}
              </option>
            ))}
          </select>
        </label>
      )}
      <label class="field-inline">
        <span class="field-inline__k">IP</span>
        <input
          class="input mono lg-ipin"
          type="search"
          value={ipInput}
          onInput={(e) => setIpInput(e.currentTarget.value)}
        />
      </label>
      <label class="field-inline">
        <span class="field-inline__k">编号</span>
        <input
          class="input mono lg-ridin"
          type="search"
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

  // 往下看了、展开了某行、或不在第 1 页：冻住（请求最频繁，新行会把正在看的行一路挤下去）
  const top = useRef<HTMLDivElement>(null);
  const scrolled = useScrolledPast(top);
  const live = useFrozenList(data?.logs, (l) => l.id, {
    frozen: scrolled || open.size > 0 || page > 1,
    key: qs.toString(),
    pending,
  });
  const rows = live.rows ?? [];
  return (
    <div class="stack">
      <div class="lg-bar">
        {narrow ? (
          <details class="lg-filters">
            <summary class="btn">筛选</summary>
            {filters}
          </details>
        ) : (
          filters
        )}
      </div>

      {error && !data ? (
        <LoadError error={error} />
      ) : !data ? (
        <Skeleton h="240px" />
      ) : (
        <>
          {data.summary && <Summary s={data.summary} />}
          <div ref={top} class={cx("card lg-listcard", pending && "xa-busy")} aria-busy={pending}>
            <FreshBar fresh={live.fresh} changed={live.changed} onApply={live.apply} anchor={top} />
            {rows.length === 0 ? (
              <Empty title="暂无记录" />
            ) : narrow ? (
              <ul class="lg-cards" aria-label="请求日志">
                {rows.map((l) => (
                  <li key={l.id} class="lg-card">
                    <div class="lg-card__top">
                      <span class="small muted">{fullTime(l.ts)}</span>
                      <Status l={l} onFilter={set} />
                    </div>
                    <div class="lg-card__meta">
                      <Who l={l} memberOf={memberOf} onFilter={set} />
                      <What l={l} />
                    </div>
                    <div class="lg-card__meta small">
                      <Timing l={l} />
                      <Cost l={l} />
                    </div>
                    <div class="lg-card__meta small">
                      <Ip l={l} onFilter={set} />
                      <Client l={l} />
                    </div>
                    <details
                      class="lg-card__more"
                      open={open.has(l.id)}
                      onToggle={(e) => setRowOpen(l.id, e.currentTarget.open)}
                    >
                      <summary class="small">详情</summary>
                      <More l={l} />
                    </details>
                  </li>
                ))}
              </ul>
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
                      <th scope="col">IP</th>
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
                            <Who l={l} memberOf={memberOf} onFilter={set} />
                          </td>
                          <td>
                            <What l={l} />
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
                            <Ip l={l} onFilter={set} />
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
                                <More l={l} />
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
              sizes={SIZES}
              onPage={(p) => set({ page: p === 1 ? null : String(p) })}
              onSize={(n) => set({ per: n === 20 ? null : String(n), page: null })}
            />
          </div>
        </>
      )}
    </div>
  );
}
