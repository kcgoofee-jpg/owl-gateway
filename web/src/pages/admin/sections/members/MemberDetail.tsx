/** 成员展开后的详情：席位 / 要处理 → 规则与资料 → 日常操作。今日额度只在列表进度条里。 */
import { ExternalLink, Plus } from "lucide-preact";
import { useEffect, useState } from "preact/hooks";
import { expiresText, fullTime, limitText, num, rel } from "../../../../lib/format";
import { Badge, Button, copyText, Menu, toast } from "../../../../ui";
import { refresh, useAdmin } from "../../data";
import {
  exceptionCount,
  FEATURE_FIELDS,
  mergeOverrides,
  type PatronView,
  type PolicySnapshot,
  policyLabel,
} from "../member-policy";
import type { SharePage, UpstreamPage } from "../upstream/core";
import { call } from "./actions";
import {
  type Issue,
  type IssueAction,
  memberIssues,
  memberLabel,
  memberLogsHref,
  memberVerdict,
  userTags,
} from "./logic";
import type { KeyRow, MemberRow } from "./types";

export type DetailActions = {
  edit: (m: MemberRow) => void;
  resetRules: (m: MemberRow) => void;
  metadata: (m: MemberRow) => void;
  issue: (m: MemberRow, a: IssueAction) => void;
  addTag: (m: MemberRow) => void;
  dm: (m: MemberRow) => void;
  removeTag: (m: MemberRow, tag: string) => void;
  coupon: (m: MemberRow) => void;
  resetQuota: (m: MemberRow) => void;
  disable: (m: MemberRow) => void;
  regenerate: (m: MemberRow) => void;
  remove: (m: MemberRow, ban: boolean) => void;
};

/* 额度一行：0 = 停用，9999 = 不限（见 format.limitText）。 */

const LEVEL_TEXT = { bad: "紧急", warn: "注意", info: "提示" } as const;
const ISSUE_BTN: Record<IssueAction["kind"], string> = {
  "share-clear": "误判，解除",
  edit: "改到期时间",
  ack: "没问题",
  "tag-del": "查完了，移除标记",
};

function IssueList(props: { m: MemberRow; issues: Issue[]; act: DetailActions }) {
  // 只有真的需要动手的事项才显示；纯提示（info）不再占一块地方。
  if (!props.issues.some((x) => x.level !== "info")) return null;
  return (
    <section class="mb-sec" aria-label="要处理">
      <h4 class="mb-h4">要处理</h4>
      <ul class="mb-issues">
        {props.issues.map((i) => (
          <li key={i.id} class={`mb-issue is-${i.level}`}>
            <Badge tone={i.level === "bad" ? "danger" : i.level === "warn" ? "warn" : "info"}>
              {LEVEL_TEXT[i.level]}
            </Badge>
            <div class="mb-issue__t">
              <b>{i.title}</b>
              <span class="small muted">{i.detail}</span>
            </div>
            <div class="mb-issue__a">
              {i.actions.map((a) => (
                <Button key={a.kind} size="sm" onClick={() => props.act.issue(props.m, a)}>
                  {ISSUE_BTN[a.kind]}
                </Button>
              ))}
            </div>
          </li>
        ))}
      </ul>
    </section>
  );
}

export function MemberDetail(props: { m: MemberRow; now: number; act: DetailActions }) {
  const { m, now, act } = props;
  const [showKey, setShowKey] = useState(false);
  // Key 详情（凭据 / 累计用量 / 生效策略）按需取：列表首屏不再整包拉全部 Key。
  // 成员对象里的 key_count==0 时不必请求。
  const detail = useAdmin<{ key: KeyRow }>(m.key_count > 0 ? `/admin/api/keys/${m.id}` : null);
  const k = m.k ?? detail.data?.key;
  const verdict = memberVerdict(m, now);
  const dc = m.discord;
  const dcId = dc ? String(dc.id) : m.discord_id ? String(m.discord_id) : "";
  const tags = userTags(m);
  const profile = dc?.profile_url?.startsWith("https://discord.com/") ? dc.profile_url : "";
  const issues = memberIssues(m, now);

  if (!k) {
    return (
      <div class="mb-detail">
        <p class="mb-verdict">{verdict.title}</p>
        {m.key_count === 0 ? (
          <p class="small muted">该成员没有 Key。</p>
        ) : detail.loading ? (
          <p class="small muted" role="status">
            正在读取 Key 详情…
          </p>
        ) : (
          <div>
            <p class="small muted" role="alert">
              Key 详情读取失败，请重试。
            </p>
            <Button size="sm" onClick={refresh}>
              重试
            </Button>
          </div>
        )}
      </div>
    );
  }
  const policy = k.policy.effective;
  const exceptions = exceptionCount(k.policy.overrides);
  return (
    <div class="mb-detail">
      {verdict.title || verdict.sub ? (
        <div>
          {verdict.title && <p class="mb-verdict">{verdict.title}</p>}
          {verdict.sub && <p class="small muted">{verdict.sub}</p>}
        </div>
      ) : null}

      {k.policy.owner_image && (
        <section class="mb-patron" aria-label="站长生图权限">
          <h4 class="mb-h4">站长生图</h4>
          <OwnerImageButtons subject={dcId || ""} enabled={k.policy.owner_image.enabled} />
        </section>
      )}
      {dcId && <PatronSeatButtons subject={dcId} patron={k.policy.patron} />}
      {issues.length > 0 && <IssueList m={m} issues={issues} act={act} />}

      <div class="mb-cols">
        <section class="mb-sec" aria-label="有效规则">
          <h4 class="mb-h4">
            有效规则
            {exceptions > 0 && <span class="small muted"> · {exceptions} 项例外</span>}
          </h4>
          <p class="mb-facts">
            <span>V4.5 {limitText(policy.image, policy.daily_images)}</span>
            <span>V5 {limitText(policy.image && policy.v5, policy.daily_v5)}</span>
            <span>
              Anlas {num(policy.daily_anlas)}/天 · {num(policy.monthly_anlas)}/月
            </span>
            <span>RPM {policy.rpm}</span>
          </p>
          <p class="mb-chips">
            {FEATURE_FIELDS.filter((name) => policy[name]).map((name) => (
              <span class="xa-chip" key={name}>
                {policyLabel(name)}
              </span>
            ))}
            {dcId ? (
              <ShareExemptToggle subject={dcId} policy={k.policy} />
            ) : (
              <span class="small muted">没有绑定 Discord</span>
            )}
            {exceptions > 0 && !k.policy.owner_image && (
              <Button
                size="sm"
                variant="ghost"
                disabled={!dcId}
                title={dcId ? undefined : "没有绑定 Discord，无成员级规则可恢复"}
                onClick={() => act.resetRules(m)}
              >
                恢复默认
              </Button>
            )}
          </p>
        </section>

        <section class="mb-sec" aria-label="资料">
          <h4 class="mb-h4">资料</h4>
          <p class="mb-facts">
            <span>{dcId ? (dc?.username ? `@${dc.username}` : dcId) : "待领取"}</span>
            <span title={fullTime(m.registered_at)}>
              {m.discord_id ? "领取" : "创建"} {m.registered_at ? rel(m.registered_at, now) : "—"}
            </span>
            <span>到期 {expiresText(k.expires_at, now)}</span>
          </p>
          <p class="mb-keyrow">
            <code class="mb-key__v">{showKey ? k.token : "••••••••"}</code>
            <Button size="sm" variant="ghost" onClick={() => setShowKey(!showKey)}>
              {showKey ? "隐藏" : "显示"}
            </Button>
            <Button
              size="sm"
              variant="ghost"
              aria-label={`复制 ${memberLabel(m)} 的 Key`}
              onClick={async () => {
                const ok = await copyText(k.token);
                toast(ok ? "Key 已复制" : "复制失败，请显示 Key 后手动复制", { error: !ok });
              }}
            >
              复制
            </Button>
            {dcId && (
              <Button
                size="sm"
                variant="ghost"
                onClick={async () => {
                  const ok = await copyText(dcId);
                  toast(ok ? "Discord ID 已复制" : "复制失败，请手动选择文字", { error: !ok });
                }}
              >
                复制 ID
              </Button>
            )}
            {profile && (
              <a
                class="btn btn--sm btn--ghost"
                href={profile}
                target="_blank"
                rel="noopener noreferrer"
                aria-label={`在 Discord 打开 ${memberLabel(m)} 的主页（新标签页）`}
              >
                Discord <ExternalLink size={12} aria-hidden="true" />
              </a>
            )}
            <Button size="sm" variant="ghost" onClick={() => act.metadata(m)}>
              改到期
            </Button>
          </p>
          <p class="mb-chips">
            {tags.map((t) => (
              <span
                class="xa-chip"
                key={t.tag}
                title={[t.note, t.by, fullTime(t.ts)].filter(Boolean).join(" · ")}
              >
                {t.tag}
                <button
                  type="button"
                  aria-label={`移除标签 ${t.tag}`}
                  onClick={() => act.removeTag(m, t.tag)}
                >
                  ×
                </button>
              </span>
            ))}
            <Button size="sm" variant="ghost" onClick={() => act.addTag(m)}>
              <Plus size={12} aria-hidden="true" />
              标签
            </Button>
          </p>
        </section>
      </div>

      <div class="mb-foot">
        <Button size="sm" onClick={() => act.resetQuota(m)}>
          重置今日额度{m.reset_today?.count ? `（${m.reset_today.count}）` : ""}
        </Button>
        <a class="btn btn--sm btn--ghost" href={memberLogsHref(m, { status: "error" })}>
          出错
        </a>
        <Button size="sm" onClick={() => act.coupon(m)}>
          发重置券
        </Button>
        <span class="spacer" />
        <Button
          size="sm"
          disabled={!dcId || k.policy.owner_image?.enabled === false}
          onClick={() => act.edit(m)}
        >
          成员例外
        </Button>
        <Menu
          button={(p) => (
            <Button size="sm" variant="ghost" {...p}>
              更多
            </Button>
          )}
          items={[
            ...(dcId ? [{ label: "私信（奶妹发）", onSelect: () => act.dm(m) }] : []),
            { label: "重新生成 Key（旧的立即失效）", onSelect: () => act.regenerate(m) },
            { label: "停用", onSelect: () => act.disable(m) },
            {
              label: dcId ? "删除（可以重新领取）" : "删除",
              onSelect: () => act.remove(m, false),
              danger: true,
            },
            ...(!dcId
              ? []
              : [{ label: "删除并永久禁止领取", onSelect: () => act.remove(m, true), danger: true }]),
          ]}
        />
      </div>
    </div>
  );
}

/** 成员级「分享豁免」快速开关：写进成员例外的 share_exempt，与例外表单、网关检查同一份状态。 */
function ShareExemptToggle({ subject, policy }: { subject: string; policy: PolicySnapshot }) {
  const [pending, setPending] = useState(false);
  const enabled = policy.effective.share_exempt === true;
  const toggle = async () => {
    if (pending) return;
    setPending(true);
    const overrides = mergeOverrides(
      policy.defaults,
      policy.overrides,
      { share_exempt: !enabled },
      new Set(),
    );
    const r = await call("PUT", `/admin/api/members/${encodeURIComponent(subject)}/rules`, overrides);
    setPending(false);
    if (!r.ok) toast(r.error, { error: true });
    else toast(enabled ? "已恢复分享要求检查" : "已豁免该成员的分享要求");
  };
  return (
    <label class="mb-patron__jump" title="跳过「先分享再用图」检查；额度、封禁与其他限制不受影响">
      <input type="checkbox" checked={enabled} disabled={pending} onChange={toggle} />
      豁免
    </label>
  );
}

export function PatronSeatButtons({ subject, patron }: { subject: string; patron?: PatronView | undefined }) {
  const upstream = useAdmin<UpstreamPage>("/admin/api/upstream");
  const shares = useAdmin<SharePage>("/admin/api/share-upstreams");
  const [pending, setPending] = useState(false);
  const [error, setError] = useState("");
  const [password, setPassword] = useState("");
  const [tokenId, setTokenId] = useState(patron?.bound_token_id ?? "");
  const [jump, setJump] = useState(patron?.queue_jump === true);
  const [selfService, setSelfService] = useState(patron?.self_service !== false);
  const [mode, setMode] = useState<string>(patron?.portal_mode ?? "basic");
  const enabled = patron?.enabled === true;
  useEffect(() => {
    setTokenId(patron?.bound_token_id ?? "");
    setJump(patron?.queue_jump === true);
    setSelfService(patron?.self_service !== false);
    setMode(patron?.portal_mode ?? "basic");
  }, [patron?.bound_token_id, patron?.queue_jump, patron?.self_service, patron?.portal_mode]);
  const save = async (body: {
    enabled: boolean;
    queue_jump?: boolean;
    self_service?: boolean;
    portal_mode?: string;
    bound_token_id?: string;
    reset_password?: boolean;
  }) => {
    setPending(true);
    setError("");
    const result = await call<{ portal_password?: string }>(
      "PUT",
      `/admin/api/members/${encodeURIComponent(subject)}/patron`,
      body,
    );
    setPending(false);
    if (!result.ok) {
      setError(result.error);
      return;
    }
    const next = result.data?.portal_password ?? "";
    setPassword(next);
    toast(body.enabled ? (next ? "赞助席已开启，请把密码交给对方" : "赞助席已更新") : "赞助席已关闭");
  };
  const accounts = upstream.data?.accounts ?? [];
  const shareAccounts = shares.data?.accounts ?? [];
  return (
    <section class="mb-patron" aria-label="赞助席">
      <h4 class="mb-h4">赞助席</h4>
      {enabled && (
        <Badge tone="ok" title={patron?.has_password ? "门户密码已有，需要时再重置" : "还没有门户密码"}>
          已开
        </Badge>
      )}
      {enabled && (
        <>
          <label class="mb-patron__jump" title="勾上后插队立即生效；对方走 /patron，看不到后台">
            <input
              type="checkbox"
              checked={jump}
              disabled={pending}
              onChange={(e) => setJump((e.target as HTMLInputElement).checked)}
            />
            插队
          </label>
          <label
            class="mb-patron__jump"
            title="自助总开关：关闭后对方只能看状态，不能改功能开关，也不能管理 Key"
          >
            <input
              type="checkbox"
              checked={selfService}
              disabled={pending}
              onChange={(e) => setSelfService((e.target as HTMLInputElement).checked)}
            />
            自助
          </label>
          <label
            class="field-inline"
            title="绑定捐赠 Token 仅用于门户管理与来源标签；捐赠号和其他上游一样进全站共享池"
          >
            <span class="field-inline__k">捐赠号</span>
            <select
              class="select"
              value={tokenId}
              disabled={pending}
              onChange={(e) => setTokenId((e.target as HTMLSelectElement).value)}
            >
              <option value="">不绑定</option>
              {accounts.map((a) => (
                <option key={a.token_id} value={a.token_id}>
                  账号 {a.position} · {a.token}
                  {a.donated ? " · 捐赠" : ""}
                </option>
              ))}
              {shareAccounts.map((a) => (
                <option key={a.token_id} value={a.token_id}>
                  拼车 {a.position} · {a.token}
                </option>
              ))}
            </select>
          </label>
          <label class="field-inline" title="基础：功能开关 + 用量查看；完整：另可自助启停、更换捐赠 Key">
            <span class="field-inline__k">门户模式</span>
            <select
              class="select"
              value={mode}
              disabled={pending}
              onChange={(e) => setMode((e.target as HTMLSelectElement).value)}
            >
              <option value="basic">基础（开关 + 用量）</option>
              <option value="full">完整（另可启停 / 换 Key）</option>
            </select>
          </label>
        </>
      )}
      <span class="mb-inline">
        {enabled ? (
          <>
            <Button
              size="sm"
              disabled={pending}
              onClick={() =>
                void save({
                  enabled: true,
                  queue_jump: jump,
                  self_service: selfService,
                  portal_mode: mode,
                  bound_token_id: tokenId,
                })
              }
            >
              保存
            </Button>
            <Button
              size="sm"
              variant="ghost"
              disabled={pending}
              onClick={() =>
                void save({
                  enabled: true,
                  queue_jump: jump,
                  self_service: selfService,
                  portal_mode: mode,
                  bound_token_id: tokenId,
                  reset_password: true,
                })
              }
            >
              重置密码
            </Button>
            <Button
              size="sm"
              variant="ghost"
              disabled={pending}
              onClick={() => void save({ enabled: false })}
            >
              关闭
            </Button>
          </>
        ) : (
          <Button
            size="sm"
            disabled={pending}
            title="开启后默认插队，可再绑捐赠号"
            onClick={() =>
              void save({
                enabled: true,
                queue_jump: true,
                self_service: selfService,
                portal_mode: mode,
                bound_token_id: tokenId,
              })
            }
          >
            开启
          </Button>
        )}
      </span>
      {password && (
        <p class="mb-patron__pw">
          <code class="mb-key__v">{password}</code>
          <Button
            size="sm"
            variant="ghost"
            onClick={async () => {
              const ok = await copyText(password);
              toast(ok ? "门户密码已复制（只显示这一次）" : "复制失败，请手动选中");
            }}
          >
            复制密码
          </Button>
        </p>
      )}
      {error && (
        <p role="alert" class="xa-formerr">
          {error}
        </p>
      )}
    </section>
  );
}

export function OwnerImageButtons({ subject, enabled }: { subject: string; enabled: boolean }) {
  const [pending, setPending] = useState(false);
  const [error, setError] = useState("");
  const change = async (next: boolean) => {
    setPending(true);
    setError("");
    const result = await call("PUT", `/admin/api/members/${encodeURIComponent(subject)}/image-access`, {
      enabled: next,
    });
    setPending(false);
    if (!result.ok) setError(result.error);
    else toast(next ? "全部生图功能已启用" : "已恢复普通默认");
  };
  return (
    <span class="mb-inline">
      {enabled ? (
        <Button size="sm" disabled={pending} onClick={() => void change(false)}>
          恢复普通默认
        </Button>
      ) : (
        <Button size="sm" disabled={pending} onClick={() => void change(true)}>
          启用全部生图功能
        </Button>
      )}
      {error && (
        <p role="alert" class="xa-formerr">
          {error}
        </p>
      )}
    </span>
  );
}
