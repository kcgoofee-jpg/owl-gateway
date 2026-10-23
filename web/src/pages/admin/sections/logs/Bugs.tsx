/**
 * Bug 追踪：按特征归并的错误。可按级别筛选、含已处理、逐条标记已处理、全部标记已处理（有确认）。
 * 网关会自动关闭不用处理的（网络断开 / 上游故障 / 非本站脚本，以及久未再出现的）：已处理的行带「自动处理」和原因；
 * 再出现会以「复发」重新打开。
 */
import { useMemo, useRef, useState } from "preact/hooks";
import { fullTime, shortTime } from "../../../../lib/format";
import { Badge, Button, copyText, cx, Skeleton, toast } from "../../../../ui";
import { useAdmin } from "../../Admin";
import { ConfirmDialog, LoadError, TableWrap, useShowMore } from "../_kit";
import { FreshBar, useFrozenList, useScrolledPast } from "../live";
import { call } from "../members/actions";
import { Empty, NARROW, useMedia } from "../members/shared";
import { autoReason, bugSource } from "./format";
import { useQueryState } from "./query";
import type { BugRow, BugsResp } from "./types";

const NOTE =
  "注意级 24 小时、错误级 7 天没再出现会自动关闭；再出现会以「复发」重新打开。网络断开、上游故障、非本站脚本的报错不记为 Bug。";

function Detail(props: { b: BugRow }) {
  const { b } = props;
  const text = `${b.detail || "（无堆栈）"}${b.last_path ? `\n路径：${b.last_path}` : ""}${b.key_name ? `\nKey：${b.key_name}` : ""}`;
  return (
    <details class="lg-bug">
      <summary>{b.title}</summary>
      <div class="lg-bug__body">
        <pre class="mono lg-pre">{text}</pre>
        <Button
          size="sm"
          onClick={async () => {
            const ok = await copyText(text);
            toast(ok ? "堆栈已复制" : "复制失败，请手动选择文字", { error: !ok });
          }}
        >
          复制堆栈
        </Button>
      </div>
    </details>
  );
}

/** 已处理：站长点的只写「已处理」；网关自动关的带「自动处理」徽标和原因。 */
function Resolved(props: { b: BugRow }) {
  const { b } = props;
  if (!b.auto_resolved) return <span class="muted small">已处理</span>;
  const why = autoReason(b.auto_resolved);
  return (
    <span class="lg-auto">
      <Badge title={`${why}（${fullTime(b.resolved_at)} 自动关闭）`}>自动处理</Badge>
      <span class="muted small">{why}</span>
    </span>
  );
}

export function Bugs() {
  const { query, set } = useQueryState();
  const narrow = useMedia(NARROW);
  const level = query.level === "error" || query.level === "warn" ? query.level : "";
  const all = query.all === "1";
  const { data, error } = useAdmin<BugsResp>(`/admin/api/errors${all ? "?all=true" : ""}`);
  const [confirmAll, setConfirmAll] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);
  // 往下看了：冻住，新冒出来的 Bug 攒着（不然正在看的那条会被挤走）
  const top = useRef<HTMLDivElement>(null);
  const scrolled = useScrolledPast(top);
  const latest = useMemo(
    () => (data ? (level ? data.errors.filter((e) => e.level === level) : data.errors) : null),
    [data, level],
  );
  const live = useFrozenList(latest, (b) => b.sig, { frozen: scrolled, key: `${level}|${all}` });

  const rows = live.rows ?? [];
  const { shown, more } = useShowMore(rows);

  if (error && !data) return <LoadError error={error} />;
  if (!data) return <Skeleton h="240px" />;

  const errCount = data.errors.filter((e) => !e.resolved_at && e.level === "error").length;

  const resolve = async (b: BugRow) => {
    setBusy(b.sig);
    const r = await call("POST", `/admin/api/errors/${encodeURIComponent(b.sig)}/resolve`);
    setBusy(null);
    if (!r.ok) toast(r.error, { error: true });
  };
  const rid = (b: BugRow) =>
    b.last_rid ? (
      <a href={`/admin/logs/usage?rid=${b.last_rid}`} class="mono" title="跳转到请求日志查看对应请求">
        {b.last_rid}
      </a>
    ) : (
      "—"
    );

  return (
    <div class="stack">
      <p class="small muted lg-note">{NOTE}</p>
      <div class="xa-toolbar">
        <label class="field-inline">
          <span class="field-inline__k">级别</span>
          <select class="select" value={level} onChange={(e) => set({ level: e.currentTarget.value })}>
            <option value="">全部级别</option>
            <option value="error">仅错误</option>
            <option value="warn">仅注意</option>
          </select>
        </label>
        <label class="xa-check lg-check">
          <input
            type="checkbox"
            checked={all}
            onChange={(e) => set({ all: e.currentTarget.checked ? "1" : null })}
          />
          <span>含已处理</span>
        </label>
        <span class="spacer" />
        <span class="small muted">
          {data.open
            ? `${data.open} 个未处理${errCount ? `（${errCount} 个错误）` : "（均为注意）"}`
            : "暂无未处理的 Bug"}
        </span>
        <Button
          variant="danger"
          disabled={!data.open}
          title="把当前所有未处理的 Bug 标记为已处理"
          onClick={() => setConfirmAll(true)}
        >
          全部标记已处理
        </Button>
      </div>

      <div ref={top} class="card lg-listcard">
        <FreshBar fresh={live.fresh} changed={live.changed} onApply={live.apply} anchor={top} unit="个" />
        {rows.length === 0 ? (
          <Empty title={all ? "暂无任何 Bug 记录" : "暂无未处理的 Bug"} />
        ) : narrow ? (
          <ul class="lg-cards" aria-label="Bug 列表">
            {shown.map((b) => (
              <li key={b.sig} class={cx("lg-card", !!b.resolved_at && "is-done")}>
                <div class="lg-card__top">
                  <Badge tone={b.level === "error" ? "danger" : "warn"}>
                    {b.level === "error" ? "错误" : "注意"}
                  </Badge>
                  <span class="small muted">{bugSource(b.source)}</span>
                  <span class="spacer" />
                  <span class="small muted">{b.count} 次</span>
                </div>
                <Detail b={b} />
                <div class="small muted">
                  首 {fullTime(b.first_seen)} · 近 {fullTime(b.last_seen)} · 编号 {rid(b)}
                </div>
                {b.resolved_at ? (
                  <Resolved b={b} />
                ) : (
                  <div>
                    <Button size="sm" disabled={busy === b.sig} onClick={() => resolve(b)}>
                      {busy === b.sig ? "处理中…" : "标记已处理"}
                    </Button>
                  </div>
                )}
              </li>
            ))}
          </ul>
        ) : (
          <TableWrap label="Bug 追踪">
            <table class="table lg-table">
              <caption class="sr-only">Bug 追踪</caption>
              <thead>
                <tr>
                  <th scope="col">级别</th>
                  <th scope="col">来源</th>
                  <th scope="col">内容</th>
                  <th scope="col" class="num">
                    次数
                  </th>
                  <th scope="col">首次 / 最近</th>
                  <th scope="col">最近错误编号</th>
                  <th scope="col">
                    <span class="sr-only">操作</span>
                  </th>
                </tr>
              </thead>
              <tbody>
                {shown.map((b) => (
                  <tr key={b.sig} class={cx(!!b.resolved_at && "lg-done")}>
                    <td>
                      <Badge tone={b.level === "error" ? "danger" : "warn"}>
                        {b.level === "error" ? "错误" : "注意"}
                      </Badge>
                    </td>
                    <td class="lg-src">{bugSource(b.source)}</td>
                    <td class="lg-bugcell">
                      <Detail b={b} />
                    </td>
                    <td class="num">{b.count}</td>
                    <td class="nowrap small" title={`${fullTime(b.first_seen)} / ${fullTime(b.last_seen)}`}>
                      <span class="muted">首</span> {shortTime(b.first_seen)}
                      <br />
                      <span class="muted">近</span> {shortTime(b.last_seen)}
                    </td>
                    <td>{rid(b)}</td>
                    <td>
                      {b.resolved_at ? (
                        <Resolved b={b} />
                      ) : (
                        <Button size="sm" disabled={busy === b.sig} onClick={() => resolve(b)}>
                          {busy === b.sig ? "处理中…" : "标记已处理"}
                        </Button>
                      )}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </TableWrap>
        )}
        {more}
      </div>

      {confirmAll && (
        <ConfirmDialog
          title="全部标记已处理"
          confirmText="全部标记"
          danger
          onClose={() => setConfirmAll(false)}
          onConfirm={async () => {
            const r = await call<{ count: number }>("POST", "/admin/api/errors/resolve-all");
            if (!r.ok) return r.error;
            toast(`已标记 ${r.data.count} 个 Bug 为已处理`);
            return undefined;
          }}
        >
          <p>确定将当前 {data.open} 个未处理的 Bug 全部标记为已处理？之后再出现会作为「复发」重新提醒。</p>
        </ConfirmDialog>
      )}
    </div>
  );
}
