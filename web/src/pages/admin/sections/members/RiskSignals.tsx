/**
 * 成员详情里的风险线索：反滥用扫描给这个成员的信号（按 Discord 账号查），展开详情时才加载。
 * 确认 = 给涉及的 Key 打「风险线索」标签（不封禁）；忽略 = 放行，分数再涨 10 以上会重新打开；
 * 重新打开 = 回到待复核。封禁是独立的显式高危操作，不在这里。
 */
import { useEffect, useState } from "preact/hooks";
import { fullTime, rel } from "../../../../lib/format";
import { Badge, Button, Notice, toast } from "../../../../ui";
import { ConfirmDialog, Loading } from "../_kit";
import { reviewSignal } from "./actions";
import { Pager, Seg, usePageData } from "./shared";
import type { AbuseResp, AbuseSignal } from "./types";

const PER = 10;

export type RiskAction = "confirm" | "dismiss" | "reopen";
export const RISK_ACTION_TEXT: Record<RiskAction, string> = {
  confirm: "确认并标记",
  dismiss: "忽略",
  reopen: "重新打开",
};

const STATUS_TEXT: Record<AbuseSignal["status"], string> = {
  open: "待复核",
  confirmed: "已确认",
  dismissed: "已忽略",
  expired: "已过期",
};
const STATUS_TONE: Record<AbuseSignal["status"], "warn" | "danger" | "info"> = {
  open: "warn",
  confirmed: "danger",
  dismissed: "info",
  expired: "info",
};

/** 各状态可用的复核动作：待复核 → 确认 / 忽略；其余 → 重开（已确认不直接忽略，先重开再决定，少一个误点清状态的机会）。 */
export const riskActions = (status: AbuseSignal["status"]): RiskAction[] =>
  status === "open" ? ["confirm", "dismiss"] : ["reopen"];

type StatusFilter = "open" | "confirmed" | "dismissed" | "all";
const TABS: Array<{ id: StatusFilter; label: string; count: keyof AbuseResp["counts"] }> = [
  { id: "open", label: "待复核", count: "open" },
  { id: "confirmed", label: "已确认", count: "confirmed" },
  { id: "dismissed", label: "已忽略", count: "dismissed" },
  { id: "all", label: "全部", count: "total" },
];
const EMPTY_TEXT: Record<StatusFilter, string> = {
  open: "没有待复核的风险线索。",
  confirmed: "没有已确认的线索。",
  dismissed: "没有已忽略的线索。",
  all: "这个成员没有风险线索。",
};

function SignalCard(props: { s: AbuseSignal; now: number; busy: boolean; onAct: (a: RiskAction) => void }) {
  const { s, now } = props;
  const evidence = s.detail.evidence ?? [];
  const seen = new Set<string>();
  const memberLine = s.members
    .filter((mm) => !seen.has(mm.discord_id) && seen.add(mm.discord_id))
    .map((mm) => `${mm.display_name || mm.username || mm.discord_id}${mm.banned ? "（已封禁）" : ""}`)
    .join("、");
  const keyLine = s.accounts
    .map((a) => `#${a.key_id} ${a.discord_name ?? a.name ?? "?"}${a.deleted ? "（已删）" : ""}`)
    .join("、");
  return (
    <li class={`mb-issue is-${s.status === "open" ? "warn" : "info"}`}>
      <Badge tone={STATUS_TONE[s.status]}>{STATUS_TEXT[s.status]}</Badge>
      <div class="mb-issue__t">
        <b>
          {s.kind_label}
          <span class="small muted" title="风险分只用于排序和重开阈值，不是违规结论">
            {" "}
            · 风险分 {Math.round(s.score)}
          </span>
        </b>
        {s.detail.summary && <span class="small">{s.detail.summary}</span>}
        {evidence.length > 0 && (
          <span class="small muted">{evidence.map((x) => `${x.k}：${String(x.v)}`).join("；")}</span>
        )}
        {memberLine && <span class="small muted">账号：{memberLine}</span>}
        {keyLine && <span class="small muted">Key：{keyLine}</span>}
        <span class="small muted">
          首次 <span title={fullTime(s.first_ts)}>{rel(s.first_ts, now)}</span> · 最近{" "}
          <span title={fullTime(s.last_ts)}>{rel(s.last_ts, now)}</span>
        </span>
      </div>
      <div class="mb-issue__a">
        {riskActions(s.status).map((a) => (
          <Button key={a} size="sm" disabled={props.busy} onClick={() => props.onAct(a)}>
            {RISK_ACTION_TEXT[a]}
          </Button>
        ))}
      </div>
    </li>
  );
}

/** 确认 / 忽略的确认框；重开不加框，直接执行。 */
function ReviewDialog(props: {
  dlg: { s: AbuseSignal; a: Exclude<RiskAction, "reopen"> };
  onClose: () => void;
}) {
  const { dlg } = props;
  const confirm = dlg.a === "confirm";
  const liveKeys = dlg.s.accounts.filter((a) => !a.deleted);
  return (
    <ConfirmDialog
      title={confirm ? `确认「${dlg.s.kind_label}」线索？` : "忽略这条线索？"}
      confirmText={confirm ? "确认线索" : "忽略"}
      onClose={props.onClose}
      onConfirm={async () => {
        const r = await reviewSignal(dlg.s.id, dlg.a);
        if (!r.ok) return r.error;
        toast(
          confirm
            ? r.data.tagged > 0
              ? `已确认，给 ${r.data.tagged} 把 Key 打了「风险线索」标签`
              : "已确认（涉及的 Key 都已删除，没有可打的标签）"
            : "已忽略；这条线索的风险分再涨 10 以上会重新打开",
        );
        return undefined;
      }}
    >
      {confirm ? (
        <>
          <p>
            只给涉及的 {liveKeys.length} 把 Key 打「风险线索」标签，<b>不会封禁</b>
            ；标签可以在成员资料里手动移除。
          </p>
          {liveKeys.length > 0 && <p class="small muted">#{liveKeys.map((k) => k.key_id).join("、#")}</p>}
        </>
      ) : (
        <>
          <p>忽略后不再出现在待复核；同一条线索的风险分比现在高 10 分以上才会重新打开。</p>
          {dlg.s.kind === "banned_link" && (
            <p>
              <b>注意：</b>这条线索关联了已封禁账号，忽略 = 放行该浏览器领取 Key。
            </p>
          )}
        </>
      )}
    </ConfirmDialog>
  );
}

export function RiskSignals(props: { dcId: string; now: number }) {
  const { dcId, now } = props;
  const [status, setStatus] = useState<StatusFilter>("open");
  const [page, setPage] = useState(1);
  const [dlg, setDlg] = useState<{ s: AbuseSignal; a: Exclude<RiskAction, "reopen"> } | null>(null);
  const { data, error, pending } = usePageData<AbuseResp>(
    `/admin/api/abuse?member=${encodeURIComponent(dcId)}&status=${status}&page=${page}&per=${PER}`,
  );
  const total = data?.total ?? 0;
  const pages = Math.max(1, Math.ceil(total / PER));
  // 确认 / 忽略后当前状态的总数可能变少：页码超出时退回最后一页
  useEffect(() => {
    if (page > pages) setPage(pages);
  }, [page, pages]);

  const counts = data?.counts;
  const reopen = async (s: AbuseSignal) => {
    const r = await reviewSignal(s.id, "reopen");
    if (!r.ok) toast(r.error, { error: true });
    else toast("已重新打开，回到待复核");
  };
  return (
    <section class="mb-sec" aria-label="风险线索">
      <h4 class="mb-h4">风险线索</h4>
      <Seg
        label="风险线索状态"
        value={status}
        options={TABS.map((t) => ({
          id: t.id,
          label: counts ? `${t.label} ${counts[t.count]}` : t.label,
        }))}
        onChange={(id) => {
          setStatus(id);
          setPage(1);
        }}
      />
      {error ? (
        <Notice tone="danger">加载失败：{error}</Notice>
      ) : !data ? (
        <Loading rows={1} />
      ) : data.signals.length === 0 ? (
        <p class="small muted">{EMPTY_TEXT[status]}</p>
      ) : (
        <>
          <ul class="mb-issues">
            {data.signals.map((s) => (
              <SignalCard
                key={s.id}
                s={s}
                now={now}
                busy={pending}
                onAct={(a) => {
                  if (a === "reopen") void reopen(s);
                  else setDlg({ s, a });
                }}
              />
            ))}
          </ul>
          <Pager page={page} pages={pages} total={total} perPage={PER} onPage={setPage} />
        </>
      )}
      {dlg && <ReviewDialog dlg={dlg} onClose={() => setDlg(null)} />}
    </section>
  );
}
