/** 成员展开后的详情：一句话结论 → 待处理 → 额度与权限 / 资料 → 日常操作。今天的用量只在列表行里显示一次。 */
import { ExternalLink, Plus } from "lucide-preact";
import { useState } from "preact/hooks";
import { dayOf, fullTime, rel, usedOf } from "../../../../lib/format";
import { Badge, Button, copyText, Menu, toast } from "../../../../ui";
import { exceptionCount, FEATURE_FIELDS, POLICY_SPECS } from "../member-policy";
import { type Issue, type IssueAction, memberIssues, memberLabel, memberVerdict, userTags } from "./logic";
import { RiskSignals } from "./RiskSignals";
import type { MemberRow } from "./types";

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

const mask = (t: string): string => `${t.slice(0, 8)}····${t.slice(-4)}`;

function IssueList(props: { m: MemberRow; issues: Issue[]; act: DetailActions }) {
  const serious = props.issues.some((x) => x.level !== "info");
  return (
    <section class="mb-sec" aria-label={serious ? "要处理" : "注意到"}>
      <h4 class="mb-h4">{serious ? "要处理" : "注意到"}</h4>
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
  const k = m.k;
  const verdict = memberVerdict(m, now);
  const [reveal, setReveal] = useState(false);
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

      {issues.length > 0 && <IssueList m={m} issues={issues} act={act} />}

      {dcId ? (
        <RiskSignals dcId={dcId} now={now} />
      ) : (
        <section class="mb-sec" aria-label="风险线索">
          <h4 class="mb-h4">风险线索</h4>
          <p class="small muted">还没绑定 Discord 账号，不能在这里按成员查线索。</p>
        </section>
      )}

      <div class="mb-cols">
        <section class="mb-sec" aria-label="有效规则">
          <h4 class="mb-h4">有效规则</h4>
          <dl class="mb-kv">
            <dt>V4.5 免费图</dt>
            <dd>{policy.image && policy.daily_images > 0 ? `${policy.daily_images} 张` : "停用"}</dd>
            <dt>V5 每天</dt>
            <dd>{policy.image && policy.v5 && policy.daily_v5 > 0 ? `${policy.daily_v5} 张` : "停用"}</dd>
            <dt>Anlas 每天</dt>
            <dd>{usedOf(Math.round(k.used.anlas), policy.daily_anlas)}</dd>
            <dt>Anlas 每月</dt>
            <dd>{policy.monthly_anlas}</dd>
            <dt>每分钟请求</dt>
            <dd>{policy.rpm} 次</dd>
            <dt>功能</dt>
            <dd>
              <span class="mb-chips">
                {FEATURE_FIELDS.filter((name) => policy[name]).map((name) => (
                  <span class="xa-chip" key={name}>
                    {POLICY_SPECS[name].label}
                  </span>
                ))}
              </span>
            </dd>
            <dt>成员例外</dt>
            <dd class="mb-key">
              {exceptions ? `${exceptions} 项` : "无"}
              {exceptions > 0 && (
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
                <code class="mb-key__v">{reveal ? k.token : mask(k.token)}</code>
                <Button size="sm" variant="ghost" aria-pressed={reveal} onClick={() => setReveal(!reveal)}>
                  {reveal ? "隐藏" : "显示"}
                </Button>
                <Button
                  size="sm"
                  variant="ghost"
                  aria-label={`复制 ${memberLabel(m)} 的 Key`}
                  onClick={async () => {
                    const ok = await copyText(k.token);
                    toast(ok ? "Key 已复制" : "复制失败，请点「显示」后手动复制", { error: !ok });
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
        {/* 生成记录 / 请求只是跳转，降成文字链接（站内链接样式），按钮留给要点的操作 */}
        <a href={`/admin/logs/usage?key=${m.all_key_ids.join(",")}`}>请求</a>
        <Button size="sm" onClick={() => act.coupon(m)}>
          发重置券
        </Button>
        <span class="spacer" />
        <Button size="sm" disabled={!dcId} onClick={() => act.edit(m)}>
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
