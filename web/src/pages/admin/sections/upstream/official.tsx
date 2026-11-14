/** 官方 NovelAI 账号：电池、官方信息、Anlas 核对。 */
import { Plus } from "lucide-preact";
import { useEffect, useState } from "preact/hooks";
import { sendJSON } from "../../../../lib/api";
import { Button, Field, Notice, toast } from "../../../../ui";
import { refreshAfterWrite, useAdmin } from "../../data";
import { actOrError, ConfirmDialog, LoadError, Loading } from "../_kit";
import { Sect } from "../system/forms";
import { AccountCard } from "./accounts";
import type { Settlement, UpstreamAccount, UpstreamPage } from "./core";
import { AccountDetails } from "./details";
import { TokenDialog } from "./tokens";

function InfoRefresh({ data }: { data: UpstreamPage }) {
  const [running, setRunning] = useState(false);
  const [retryAfter, setRetryAfter] = useState(0);
  useEffect(() => {
    setRetryAfter(data.info_refresh.retry_after ?? 0);
  }, [data.info_refresh.last_attempt_at, data.info_refresh.retry_after]);
  useEffect(() => {
    if (retryAfter <= 0) return;
    const timer = setTimeout(() => setRetryAfter((s) => Math.max(0, s - 1)), 1000);
    return () => clearTimeout(timer);
  }, [retryAfter]);
  const refresh = async () => {
    setRunning(true);
    try {
      const info = await sendJSON<{
        accounts: { last_error: string | null; last_success: number | null }[];
        last_attempt_at: number;
      }>(
        "POST",
        "/admin/api/upstream/refresh-info",
        {},
        {
          "X-NAI-Admin-CSRF": data.csrf_info,
        },
      );
      const fresh = info.accounts.filter(
        (a) => a.last_success === info.last_attempt_at && !a.last_error,
      ).length;
      toast(`本次查询成功 ${fresh}/${info.accounts.length} 个账号`, { error: fresh < info.accounts.length });
    } catch (e) {
      toast(e instanceof Error ? e.message : String(e), { error: true });
    } finally {
      setRunning(false);
      refreshAfterWrite();
    }
  };
  return (
    <Button disabled={running || retryAfter > 0 || data.accounts.length === 0} onClick={() => void refresh()}>
      {running ? "正在查询…" : retryAfter > 0 ? `${retryAfter} 秒后刷新` : "查询官方信息"}
    </Button>
  );
}

/** 结果未知的请求占着额度：逐条核对后释放（确认上游没执行才退）。 */
function Settlements({ items, csrf }: { items: Settlement[]; csrf: string }) {
  const [target, setTarget] = useState<Settlement | null>(null);
  const [reason, setReason] = useState("");
  const [checked, setChecked] = useState(false);
  if (!items.length) return null;
  return (
    <>
      <Notice tone="warn">
        <span class="small">{items.length} 条结果未知，占着额度。</span>
        <Button size="sm" onClick={() => setTarget(items[0] ?? null)}>
          核对释放
        </Button>
      </Notice>
      {target && (
        <ConfirmDialog
          title={`核对「${target.rid || target.request_id}」`}
          confirmText="确认未执行并释放"
          danger
          onClose={() => setTarget(null)}
          onConfirm={async () => {
            if (!checked) return "请先确认这张图没有执行";
            const text = reason.trim();
            if (text.length < 12) return "核对说明至少 12 个字";
            try {
              await sendJSON(
                "POST",
                `/admin/api/request-settlements/${encodeURIComponent(target.request_id)}/release`,
                { confirmed_not_executed: true, reason: text },
                { "X-NAI-Admin-CSRF": csrf },
              );
            } catch (e) {
              return e instanceof Error ? e.message : String(e);
            }
            toast("已释放预留");
            refreshAfterWrite();
            return undefined;
          }}
        >
          <p>
            释放会把预留的 {target.reserved_anlas} Anlas / {target.reserved_v5} V5
            退回额度。只在这张图确实没执行时用。
          </p>
          <Field label="核对说明（12～500 字）">
            <textarea
              class="input"
              rows={3}
              maxLength={500}
              value={reason}
              onInput={(e) => setReason(e.currentTarget.value)}
            />
          </Field>
          <label class="small">
            <input type="checkbox" checked={checked} onChange={(e) => setChecked(e.currentTarget.checked)} />{" "}
            我确认上游没有执行这张图
          </label>
        </ConfirmDialog>
      )}
    </>
  );
}

export function OfficialUpstream() {
  const { data, error, loading } = useAdmin<UpstreamPage>("/admin/api/upstream");
  const [dialog, setDialog] = useState<{ mode: "add" | "replace"; target?: UpstreamAccount } | null>(null);
  const [details, setDetails] = useState<UpstreamAccount | null>(null);
  const [del, setDel] = useState<UpstreamAccount | null>(null);
  const [toggle, setToggle] = useState<UpstreamAccount | null>(null);

  const pool = data?.accounts ?? [];
  const poolSize = data?.pool_size ?? pool.length;
  useEffect(() => {
    if (details && !pool.some((account) => account.token_id === details.token_id)) setDetails(null);
  }, [data, details, pool]);
  const usable = pool.filter((p) => p.admin_enabled && !p.disabled);
  const lastUsable =
    toggle?.admin_enabled === true && usable.filter((p) => p.token_id !== toggle.token_id).length === 0;
  const label = (p: UpstreamAccount) => `#${p.position} ${p.token}`;

  return (
    <Sect>
      <div class="section-head">
        <p>查询官方信息会刷新账号状态和电量，间隔 5 分钟。后台刷新按钮只更新网关读数。</p>
        <span class="spacer" />
        {data && <InfoRefresh data={data} />}
        <Button variant="primary" onClick={() => setDialog({ mode: "add" })}>
          <Plus size={16} aria-hidden="true" />
          添加 Token
        </Button>
      </div>
      {loading && !data && <Loading rows={3} />}
      {!data && error && <LoadError error={error} />}
      {data && !data.pool_configured && <Notice tone="danger">尚未配置上游 Token。</Notice>}
      {data && <Settlements items={data.settlements} csrf={data.csrf_token} />}
      {data && (
        <div class="stack">
          {data.accounts
            .slice()
            .sort(
              (x, y) =>
                Number(y.admin_enabled && !y.disabled) - Number(x.admin_enabled && !x.disabled) ||
                x.position - y.position,
            )
            .map((a) => (
              <AccountCard
                key={a.token_id}
                a={a}
                limit={data.fields.v5_daily_limit}
                only={poolSize <= 1}
                onDetails={setDetails}
                onReplace={(t) => setDialog({ mode: "replace", target: t })}
                onDelete={setDel}
                onToggle={setToggle}
              />
            ))}
        </div>
      )}
      {details && data && (
        <AccountDetails
          key={details.token_id}
          account={details}
          csrf={data.csrf_info}
          onClose={() => setDetails(null)}
        />
      )}
      {dialog && <TokenDialog mode={dialog.mode} target={dialog.target} onClose={() => setDialog(null)} />}

      {del && (
        <ConfirmDialog
          title="删除上游 Token？"
          confirmText="删除"
          danger
          onClose={() => setDel(null)}
          onConfirm={() =>
            actOrError(
              "DELETE",
              `/admin/api/upstream-tokens/${encodeURIComponent(del.token_id)}`,
              undefined,
              "已删除上游 Token",
            )
          }
        >
          <p>从令牌池删除 {label(del)}？它将不再接收新请求，已发出的请求继续完成。</p>
          <Notice tone="warn">删除后如需恢复，要重新粘贴这把 Token。</Notice>
        </ConfirmDialog>
      )}

      {toggle && (
        <ConfirmDialog
          title={toggle.admin_enabled ? "停用这把上游 Token？" : "启用这把上游 Token？"}
          confirmText={toggle.admin_enabled ? "停用" : "启用"}
          danger={toggle.admin_enabled}
          onClose={() => setToggle(null)}
          onConfirm={() =>
            actOrError(
              "PUT",
              `/admin/api/upstream-tokens/${encodeURIComponent(toggle.token_id)}/enabled`,
              { enabled: !toggle.admin_enabled },
              toggle.admin_enabled ? "上游 Token 已停用" : "上游 Token 已启用（401 失效仍需替换 Token）",
            )
          }
        >
          {toggle.admin_enabled ? (
            lastUsable && poolSize <= 1 ? (
              <Notice tone="danger">
                这是最后一把可用的上游 Token：停用后全站将无法出图，直到重新启用。
              </Notice>
            ) : (
              <p>停用 {label(toggle)} 后不再接新请求；手里在跑的图最多再等 8 秒。</p>
            )
          ) : (
            <p>启用 {label(toggle)} 后，它重新接收请求。已失效（401）的 Token 需要替换才能恢复。</p>
          )}
        </ConfirmDialog>
      )}
    </Sect>
  );
}
