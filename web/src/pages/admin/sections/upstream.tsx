/**
 * 上游：一次取全（GET /admin/api/upstream），然后组装。
 * 按需查询官方信息、账号完整详情与 Anlas 核对；完整详情只存在打开的弹窗中。
 */

import { Plus } from "lucide-preact";
import { useEffect, useState } from "preact/hooks";
import { sendJSON } from "../../../lib/api";
import { Button, Notice, toast } from "../../../ui";
import { tick, useAdmin } from "../Admin";
import { actOrError, ConfirmDialog, LoadError, Loading, SectionHead } from "./_kit";
import { Sect } from "./system/forms";
import { AccountCard } from "./upstream/accounts";
import type { UpstreamAccount, UpstreamPage } from "./upstream/core";
import { AccountDetails } from "./upstream/details";
import ReconcileCard from "./upstream/reconcile";
import { TokenDialog } from "./upstream/tokens";
import "./upstream.css";

/** 「查询官方信息」按钮：本地倒计时到冷却结束。 */
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
      await sendJSON(
        "POST",
        "/admin/api/upstream/refresh-info",
        {},
        {
          "X-NAI-Admin-CSRF": data.csrf_info,
        },
      );
      toast("上游账号信息已刷新");
    } catch (e) {
      toast(e instanceof Error ? e.message : String(e), { error: true });
    } finally {
      setRunning(false);
      tick.value++;
    }
  };
  return (
    <Button disabled={running || retryAfter > 0} onClick={() => void refresh()}>
      {running ? "正在查询…" : retryAfter > 0 ? `${retryAfter} 秒后刷新` : "查询官方信息"}
    </Button>
  );
}

export default function Upstream() {
  const { data, error, loading } = useAdmin<UpstreamPage>("/admin/api/upstream");
  const [dialog, setDialog] = useState<{ mode: "add" | "replace"; target?: UpstreamAccount } | null>(null);
  const [details, setDetails] = useState<UpstreamAccount | null>(null);
  const [del, setDel] = useState<UpstreamAccount | null>(null);
  const [toggle, setToggle] = useState<UpstreamAccount | null>(null);

  const pool = data?.accounts ?? [];
  useEffect(() => {
    if (details && !pool.some((account) => account.token_id === details.token_id)) setDetails(null);
  }, [data, details]);
  const usable = pool.filter((p) => p.admin_enabled && !p.disabled);
  const lastUsable =
    toggle?.admin_enabled === true && usable.filter((p) => p.token_id !== toggle.token_id).length === 0;
  const label = (p: UpstreamAccount) => `#${p.position} ${p.token}`;

  return (
    <Sect>
      <SectionHead
        title="上游"
        actions={
          <>
            {data && <InfoRefresh data={data} />}
            <Button variant="primary" onClick={() => setDialog({ mode: "add" })}>
              <Plus size={16} aria-hidden="true" />
              添加 Token
            </Button>
          </>
        }
      />
      {loading && !data && <Loading rows={3} />}
      {!data && error && <LoadError error={error} />}
      {data && !data.pool_configured && <Notice tone="danger">尚未配置上游 Token。</Notice>}
      <div class="stack">
        {(data?.accounts ?? [])
          // 能用的排前面，已停用 / 被上游禁用的沉到最后（同类里按编号）
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
              only={pool.length <= 1}
              onDetails={setDetails}
              onReplace={(t) => setDialog({ mode: "replace", target: t })}
              onDelete={setDel}
              onToggle={setToggle}
            />
          ))}
      </div>
      {data && (
        <ReconcileCard
          recon={data.reconciliation}
          settlements={data.settlements}
          csrf={data.csrf_token}
          loading={loading}
          error={error}
        />
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
            lastUsable ? (
              <Notice tone="danger">
                这是最后一把可用的上游 Token：停用后全站将无法出图，直到重新启用。
              </Notice>
            ) : (
              <p>停用 {label(toggle)} 后，它不再接收新请求；已发出的请求继续完成。</p>
            )
          ) : (
            <p>启用 {label(toggle)} 后，它重新接收请求。已失效（401）的 Token 需要替换才能恢复。</p>
          )}
        </ConfirmDialog>
      )}
    </Sect>
  );
}
