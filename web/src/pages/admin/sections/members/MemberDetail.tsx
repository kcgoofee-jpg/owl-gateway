/** 成员展开后的详情：一句话结论 → 待处理 → 额度与权限 / 资料 → 日常操作。今天的用量只在列表行里显示一次。 */
import { ExternalLink, Plus } from "lucide-preact";
import { useEffect, useState } from "preact/hooks";
import { dayOf, fullTime, rel, usedOf } from "../../../../lib/format";
import { Badge, Button, copyText, Menu, toast } from "../../../../ui";
import { useAdmin } from "../../data";
import { exceptionCount, FEATURE_FIELDS, type PatronView, policyLabel } from "../member-policy";
import type { UpstreamPage } from "../upstream/core";
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
  removeTag: (m: MemberRow, tag: string) => void;
  coupon: (m: MemberRow) => void;
  resetQuota: (m: MemberRow) => void;
  disable: (m: MemberRow) => void;
  regenerate: (m: MemberRow) => void;
  remove: (m: MemberRow, ban: boolean) => void;
};

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
  // Key 详情（凭据 / 累计用量 / 生效策略）按需取：列表首屏不再整包拉全部 Key。
  // useAdmin 跟着全局刷新节拍自动重取；成员对象里的 key_count==0 时不必请求。
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
        <p class="small muted">这把 Key 的详细信息读取失败，稍后会自动重试。</p>
      </div>
    );
  }
  const policy = k.policy.effective;
  const exceptions = exceptionCount(k.policy.overrides);
  return (
    <div class="mb-detail">
      <div>
        <p class="mb-verdict">{verdict.title}</p>
        {verdict.sub && <p class="small muted">{verdict.sub}</p>}
      </div>

      {k.policy.owner_image && (
        <section class="mb-sec" aria-label="站长生图权限">
          <h4 class="mb-h4">站长生图权限</h4>
          <p>
            {k.policy.owner_image.enabled
              ? "全部生图功能已启用，保留个人额度和全站预算。排队走插队车道，排在所有等待中的生图前面；全站排满时仍能入队。"
              : "普通测试：实时使用普通成员默认规则，站长配置已保留。"}
          </p>
          <OwnerImageButtons subject={dcId || ""} enabled={k.policy.owner_image.enabled} />
        </section>
      )}
      {dcId && (
        <section class="mb-sec" aria-label="赞助席">
          <h4 class="mb-h4">赞助席</h4>
          <PatronSeatButtons subject={dcId} patron={k.policy.patron} />
        </section>
      )}
      {issues.length > 0 && <IssueList m={m} issues={issues} act={act} />}

      <div class="mb-cols">
        <section class="mb-sec" aria-label="有效规则">
          <h4 class="mb-h4">有效规则</h4>
          <dl class="mb-kv">
            <dt>V4.5 免费图</dt>
            <dd>{policy.image && policy.daily_images > 0 ? `${policy.daily_images} 张` : "停用"}</dd>
            <dt>V5 每天</dt>
            <dd>{policy.image && policy.v5 && policy.daily_v5 > 0 ? `${policy.daily_v5} 张` : "停用"}</dd>
            <dt>Anlas 每天</dt>
            <dd>{policy.daily_anlas > 0 ? usedOf(Math.round(k.used.anlas), policy.daily_anlas) : "停用"}</dd>
            <dt>Anlas 每月</dt>
            <dd>{policy.monthly_anlas}</dd>
            <dt>每分钟请求</dt>
            <dd>{policy.rpm} 次</dd>
            <dt>功能</dt>
            <dd>
              <span class="mb-chips">
                {FEATURE_FIELDS.filter((name) => policy[name]).map((name) => (
                  <span class="xa-chip" key={name}>
                    {policyLabel(name)}
                  </span>
                ))}
              </span>
            </dd>
            <dt>成员例外</dt>
            <dd class="mb-key">
              {exceptions ? `${exceptions} 项` : "无"}
              {exceptions > 0 && !k.policy.owner_image && (
                <Button size="sm" variant="ghost" onClick={() => act.resetRules(m)}>
                  恢复默认
                </Button>
              )}
            </dd>
            {m.reset_today && m.reset_today.count > 0 && (
              <>
                <dt>今日重置</dt>
                <dd>{m.reset_today.count} 次</dd>
              </>
            )}
          </dl>
        </section>

        <section class="mb-sec" aria-label="资料">
          <h4 class="mb-h4">资料</h4>
          <dl class="mb-kv">
            <dt>Discord</dt>
            <dd>
              {dcId ? (
                <span class="mb-key">
                  <span>{dc?.username ? `@${dc.username}` : dcId}</span>
                  {profile && (
                    <a
                      class="btn btn--sm btn--ghost"
                      href={profile}
                      target="_blank"
                      rel="noopener noreferrer"
                      aria-label={`在 Discord 打开 ${memberLabel(m)} 的主页（新标签页）`}
                    >
                      打开 <ExternalLink size={12} aria-hidden="true" />
                    </a>
                  )}
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
                </span>
              ) : (
                <span class="muted">待领取</span>
              )}
            </dd>
            <dt>Key</dt>
            <dd>
              <span class="mb-key">
                <code class="mb-key__v">{k.token}</code>
                <Button
                  size="sm"
                  variant="ghost"
                  aria-label={`复制 ${memberLabel(m)} 的 Key`}
                  onClick={async () => {
                    const ok = await copyText(k.token);
                    toast(ok ? "Key 已复制" : "复制失败，请手动选中复制", { error: !ok });
                  }}
                >
                  复制
                </Button>
              </span>
            </dd>
            <dt>{m.discord_id ? "领取" : "创建"}</dt>
            <dd title={fullTime(m.registered_at)}>{m.registered_at ? rel(m.registered_at, now) : "—"}</dd>
            <dt>到期</dt>
            <dd class="mb-key">
              {k.expires_at ? dayOf(k.expires_at) : "永久"}
              <Button size="sm" variant="ghost" onClick={() => act.metadata(m)}>
                修改
              </Button>
            </dd>
            <dt>最近使用</dt>
            <dd title={fullTime(k.last_used_at)}>{k.last_used_at ? rel(k.last_used_at, now) : "从未使用"}</dd>
            <dt>标签</dt>
            <dd>
              <span class="mb-chips">
                {tags.map((t) => (
                  <span
                    class="xa-chip"
                    key={t.tag}
                    title={`${t.note ? `${t.note} · ` : ""}${t.by || ""} · ${fullTime(t.ts)}`}
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
              </span>
            </dd>
          </dl>
        </section>
      </div>

      <div class="mb-foot">
        <Button size="sm" onClick={() => act.resetQuota(m)}>
          重置今日额度{m.reset_today?.count ? `（${m.reset_today.count}）` : ""}
        </Button>
        <a class="btn btn--sm" href={memberLogsHref(m)}>
          请求记录
        </a>
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

export function PatronSeatButtons({ subject, patron }: { subject: string; patron?: PatronView | undefined }) {
  const upstream = useAdmin<UpstreamPage>("/admin/api/upstream");
  const [pending, setPending] = useState(false);
  const [error, setError] = useState("");
  const [password, setPassword] = useState("");
  const [tokenId, setTokenId] = useState(patron?.bound_token_id ?? "");
  const [jump, setJump] = useState(patron?.queue_jump ?? true);
  const enabled = patron?.enabled === true;
  useEffect(() => {
    setTokenId(patron?.bound_token_id ?? "");
    setJump(patron?.queue_jump ?? true);
  }, [patron?.bound_token_id, patron?.queue_jump]);
  const save = async (body: {
    enabled: boolean;
    queue_jump?: boolean;
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
  return (
    <>
      <p>
        {enabled
          ? `插队${jump ? "已开" : "未开"}。对方走独立入口 /patron，看不到后台。打开席位后插队立即生效。`
          : "指定后对方可以插队，并在 /patron 打开自己 Key 的功能开关。绑定捐赠 Token 后只走那把账号。"}
      </p>
      {enabled && (
        <p class="small muted">
          {patron?.has_password ? "门户密码已有，需要时再重置。" : "还没有门户密码。"}
          {patron?.bound_token_id ? " 已绑定专属上游账号。" : " 未绑定专属账号，出图仍走全站池。"}
        </p>
      )}
      <label class="mb-key">
        <span class="small muted">插队</span>
        <input
          type="checkbox"
          checked={jump}
          disabled={pending}
          onChange={(e) => setJump((e.target as HTMLInputElement).checked)}
        />
      </label>
      <label class="mb-key">
        <span class="small muted">专属 Token</span>
        <select
          class="select"
          value={tokenId}
          disabled={pending}
          onChange={(e) => setTokenId((e.target as HTMLSelectElement).value)}
        >
          <option value="">不绑定（金钱赞助，走全站池）</option>
          {accounts.map((a) => (
            <option key={a.token_id} value={a.token_id}>
              账号 {a.position} · {a.token}
              {a.exclusive ? " · 专属" : ""}
            </option>
          ))}
        </select>
      </label>
      <Button
        size="sm"
        disabled={pending || enabled}
        onClick={() => void save({ enabled: true, queue_jump: jump, bound_token_id: tokenId })}
      >
        开启席位
      </Button>
      <Button
        size="sm"
        disabled={pending || !enabled}
        onClick={() =>
          void save({ enabled: true, queue_jump: jump, bound_token_id: tokenId, reset_password: true })
        }
      >
        重置门户密码
      </Button>
      <Button
        size="sm"
        disabled={pending || !enabled}
        onClick={() => void save({ enabled: true, queue_jump: jump, bound_token_id: tokenId })}
      >
        保存绑定
      </Button>
      <Button size="sm" disabled={pending || !enabled} onClick={() => void save({ enabled: false })}>
        关闭席位
      </Button>
      {password && (
        <p class="mb-key">
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
    </>
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
    <>
      <Button size="sm" disabled={pending || enabled} onClick={() => void change(true)}>
        启用全部生图功能
      </Button>
      <Button size="sm" disabled={pending || !enabled} onClick={() => void change(false)}>
        恢复普通默认
      </Button>
      {error && (
        <p role="alert" class="xa-formerr">
          {error}
        </p>
      )}
    </>
  );
}
