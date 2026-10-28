import { useEffect, useRef, useState } from "preact/hooks";
import { fullTime } from "../../../../lib/format";
import { Button, Dialog, Notice } from "../../../../ui";
import type { UpstreamAccount } from "./core";

type Endpoint = { status: number | null; data: Record<string, unknown> | null; error: string | null };
type Details = {
  token_id: string;
  queried_at: number;
  retry_after: number;
  information: Endpoint;
  subscription: Endpoint;
  priority: Endpoint;
};
const PANELS = { information: "账号", subscription: "订阅", priority: "优先级" } as const;
const LABELS: Record<string, string> = {
  plainTextEmail: "邮箱",
  email: "邮箱",
  emailVerified: "邮箱已验证",
  emailVerificationLetterSent: "验证邮件已发送",
  accountCreatedAt: "创建时间",
  allowMarketingEmails: "营销邮件",
  banStatus: "账号状态",
  banMessage: "封禁原因",
  trialActivated: "试用已激活",
  trialActionsLeft: "试用文本次数",
  trialImagesLeft: "试用图片数",
  accountType: "账号类型",
  active: "订阅生效",
  expiresAt: "到期时间",
  tier: "订阅等级",
  isGracePeriod: "宽限期",
  isNewCustomer: "新客户",
  isPaypal: "PayPal",
  paymentProcessor: "支付渠道",
  paymentProcessorData: "支付信息",
  perks: "订阅权益",
  trainingStepsLeft: "训练步数",
  fixedTrainingStepsLeft: "订阅训练步数",
  purchasedTrainingSteps: "购买训练步数",
  usage: "用量",
  maxPriorityActions: "优先次数上限",
  nextRefillAt: "下次补充时间",
  taskPriority: "任务优先级",
  contextTokens: "上下文长度",
  moduleTrainingSteps: "模块训练步数",
  startPriority: "初始优先级",
  unlimitedMaxPriority: "最高优先级不限次",
};
const TIMES = new Set(["accountCreatedAt", "expiresAt", "nextRefillAt"]);
function accountDate(value: number): string {
  const date = new Date(value * 1000);
  return Number.isFinite(date.getTime())
    ? date.toLocaleString("zh-CN", {
        year: "numeric",
        month: "2-digit",
        day: "2-digit",
        hour: "2-digit",
        minute: "2-digit",
        second: "2-digit",
        hour12: false,
      })
    : String(value);
}
function DetailValue({ value, name, depth = 0 }: { value: unknown; name: string; depth?: number }) {
  if (value === null || value === undefined) return <>—</>;
  if (typeof value === "boolean") return <>{value ? "是" : "否"}</>;
  if (typeof value === "number")
    return <>{TIMES.has(name) && value > 0 ? accountDate(value) : String(value)}</>;
  if (typeof value === "string") return <code class="up-detail__text">{value || "—"}</code>;
  if (depth >= 3 || Array.isArray(value))
    return <pre class="up-detail__json">{JSON.stringify(value, null, 2)}</pre>;
  return (
    <dl class="up-detail__fields">
      {Object.entries(value as Record<string, unknown>).map(([key, item]) => (
        <div key={key}>
          <dt>{LABELS[key] ?? <code>{key}</code>}</dt>
          <dd>
            <DetailValue value={item} name={key} depth={depth + 1} />
          </dd>
        </div>
      ))}
    </dl>
  );
}

export function AccountDetails({
  account,
  csrf,
  onClose,
}: {
  account: UpstreamAccount;
  csrf: string;
  onClose: () => void;
}) {
  const [data, setData] = useState<Details | null>(null);
  const [panel, setPanel] = useState<keyof typeof PANELS>("information");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [remaining, setRemaining] = useState(0);
  const pending = useRef<AbortController | null>(null);
  const read = async () => {
    pending.current?.abort();
    const controller = new AbortController();
    pending.current = controller;
    setBusy(true);
    setError("");
    setData(null);
    try {
      const response = await fetch(`/admin/api/upstream/${encodeURIComponent(account.token_id)}/details`, {
        method: "POST",
        credentials: "same-origin",
        cache: "no-store",
        signal: controller.signal,
        headers: { "Content-Type": "application/json", "X-NAI-Admin-CSRF": csrf },
        body: "{}",
      });
      const body = await response.json();
      if (controller.signal.aborted) return;
      if (!response.ok) {
        const seconds = Number(response.headers.get("retry-after") ?? 0);
        if (Number.isFinite(seconds) && seconds > 0) setRemaining(Math.ceil(seconds));
        throw new Error(body.detail ?? "账号信息查询失败");
      }
      if (body.token_id !== account.token_id) throw new Error("账号已变更，请重新打开详情");
      setData(body as Details);
      setRemaining(60);
    } catch (cause) {
      if (!controller.signal.aborted) setError(cause instanceof Error ? cause.message : "账号信息查询失败");
    } finally {
      if (!controller.signal.aborted) setBusy(false);
    }
  };
  useEffect(() => {
    void read();
    return () => pending.current?.abort();
  }, [account.token_id, csrf]);
  useEffect(() => {
    if (remaining <= 0) return;
    const timer = setTimeout(() => setRemaining((value) => Math.max(0, value - 1)), 1000);
    return () => clearTimeout(timer);
  }, [remaining]);
  const current = data?.[panel];
  return (
    <Dialog
      open
      wide
      title={`账号 ${account.position} · 完整信息`}
      onClose={onClose}
      footer={
        <>
          {data && (
            <time class="small muted" dateTime={new Date(data.queried_at * 1000).toISOString()}>
              {fullTime(data.queried_at)}
            </time>
          )}
          <span class="spacer" />
          <Button disabled={busy || remaining > 0} onClick={() => void read()}>
            {busy ? "查询中…" : remaining > 0 ? `${remaining} 秒后刷新` : "刷新"}
          </Button>
          <Button onClick={onClose}>关闭</Button>
        </>
      }
    >
      <fieldset class="up-detail__tabs">
        <legend class="sr-only">官方信息分类</legend>
        {Object.entries(PANELS).map(([key, label]) => (
          <Button key={key} aria-pressed={panel === key} onClick={() => setPanel(key as keyof typeof PANELS)}>
            {label}
          </Button>
        ))}
      </fieldset>
      {busy && <p role="status">查询中…</p>}
      {error && <Notice tone="danger">{error}</Notice>}
      {current?.error && <Notice tone="warn">{current.error}</Notice>}
      {current?.data && <DetailValue value={current.data} name="root" />}
    </Dialog>
  );
}
