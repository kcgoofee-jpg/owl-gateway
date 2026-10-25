/** Admin-only cached projection from NovelAI /user/information. GET never refreshes upstream data. */

import type { ComponentChildren } from "preact";
import { useEffect, useState } from "preact/hooks";
import { sendJSON } from "../../../../lib/api";
import { fullTime } from "../../../../lib/format";
import { Badge, Button, Card, Notice, toast } from "../../../../ui";
import { tick, useAdmin } from "../../Admin";

const URL = "/admin/api/upstream-information";

export type UpstreamInformationAccount = {
  account: number;
  ban_status: string | null;
  ban_message: string | null;
  email_verified: boolean | null;
  account_created_at: number | null;
  trial_activated: boolean | null;
  trial_actions_left: number | null;
  trial_images_left: number | null;
  last_success: number | null;
  last_error: string | null;
  last_error_at: number | null;
  stale: boolean;
};
export type UpstreamInformation = {
  accounts: UpstreamInformationAccount[];
  last_attempt_at: number | null;
  cooldown_seconds: number;
  retry_after: number;
  csrf_token: string | null;
};

export function InformationAccounts(props: {
  accounts: UpstreamInformationAccount[];
  children?: ComponentChildren;
}) {
  return (
    <div class="stack">
      {props.accounts.map((a) => (
        <div class="stack-sm" key={a.account}>
          <div class="row">
            <b>账号 {a.account}</b>
            {a.ban_status == null ? (
              <Badge>封禁状态未知</Badge>
            ) : a.ban_status === "not_banned" ? (
              <Badge tone="ok">未封禁</Badge>
            ) : (
              <Badge tone="danger">{a.ban_status}</Badge>
            )}
            {a.stale && <Badge>状态陈旧</Badge>}
            {a.last_success != null && <span class="muted small">查询于 {fullTime(a.last_success)}</span>}
          </div>
          {a.ban_message && <p class="small">{a.ban_message}</p>}
          <p class="muted small">
            邮箱验证：{a.email_verified == null ? "未知" : a.email_verified ? "已验证" : "未验证"} ·
            账号创建：
            {a.account_created_at == null ? "未知" : fullTime(a.account_created_at)} · 试用：
            {a.trial_activated == null ? "未知" : a.trial_activated ? "已启用" : "未启用"}
            {a.trial_images_left != null && ` · 试用图片剩余 ${a.trial_images_left}`}
          </p>
          {a.last_error && (
            <Notice tone="warn">
              {a.last_error}（{a.last_error_at == null ? "时间未知" : fullTime(a.last_error_at)}
              ）；保留最近一次成功读数。
            </Notice>
          )}
        </div>
      ))}
      {props.children}
    </div>
  );
}

export default function InformationCard() {
  const { data, error, loading } = useAdmin<UpstreamInformation>(URL);
  const [running, setRunning] = useState(false);
  const [retryAfter, setRetryAfter] = useState(0);
  useEffect(() => {
    setRetryAfter(data?.retry_after ?? 0);
  }, [data?.last_attempt_at, data?.retry_after]);
  useEffect(() => {
    if (retryAfter <= 0) return;
    const timer = setTimeout(() => setRetryAfter((s) => Math.max(0, s - 1)), 1000);
    return () => clearTimeout(timer);
  }, [retryAfter]);
  const refresh = async () => {
    if (!data?.csrf_token) return;
    setRunning(true);
    try {
      await sendJSON<UpstreamInformation>("POST", URL, {}, { "X-NAI-Admin-CSRF": data.csrf_token });
      toast("上游账号信息已刷新");
    } catch (e) {
      toast(e instanceof Error ? e.message : String(e), { error: true });
    } finally {
      setRunning(false);
      tick.value++;
    }
  };
  return (
    <Card
      title="上游账号状态"
      sub="仅后台可见。页面自动刷新只读缓存；手动查询每 5 分钟最多一次。只保存封禁、验证和试用状态等运维字段。"
      actions={
        <Button
          disabled={!data?.csrf_token || loading || running || retryAfter > 0}
          onClick={() => void refresh()}
        >
          {running ? "正在查询…" : retryAfter > 0 ? `${retryAfter} 秒后可再次查询` : "手动查询"}
        </Button>
      }
    >
      {!data && error && <Notice tone="warn">{error}</Notice>}
      {!data && loading && <p class="muted small">正在读取本地缓存…</p>}
      {data && data.accounts.length === 0 && <p class="muted small">未配置上游账号。</p>}
      {data && <InformationAccounts accounts={data.accounts} />}
      {data?.last_attempt_at != null && (
        <p class="muted small">最近一次手动查询尝试：{fullTime(data.last_attempt_at)}</p>
      )}
    </Card>
  );
}
