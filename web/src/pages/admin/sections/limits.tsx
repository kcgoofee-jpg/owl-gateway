/** 额度与保护：每个数都是站长手动设的（10-23 起没有自动额度算法）；「上游账号现状」只显示。 */

import { X } from "lucide-preact";
import { useEffect, useMemo, useRef, useState } from "preact/hooks";
import { fullTime, withUnit } from "../../../lib/format";
import { Badge, Button, Card, SearchMultiSelect, type SearchOption } from "../../../ui";
import { act, useAdmin } from "../Admin";
import { CardPending, SectionHead, TableWrap, useBusy } from "./_kit";
import type { QuotaMode } from "./limits/core";
import {
  agoText,
  DAILY_CAP_MAX,
  dash,
  type GlobalQuotaModes,
  type Guard,
  guardSpecs,
  MONTHLY_CAP_MAX,
  QUOTA_SPECS,
  type Records,
  RUNTIME_SPECS,
  type Settings,
} from "./limits/core";
import { Seg } from "./members/shared";
import type { NumSpec } from "./system/core";
import { NumField, SaveBar, Sect, SelectField, SwitchRow, useForm } from "./system/forms";

type ModeName = Exclude<keyof GlobalQuotaModes, "public_vibe_enabled">;

/** 迁移前的旧数据可能是 unlimited：显示与编辑都折算成上限值（9999 / 每月池 99999）。 */
const boundMode = (name: ModeName, mode: QuotaMode) =>
  mode.mode === "unlimited"
    ? { mode: "limit" as const, value: name === "global_monthly_anlas" ? MONTHLY_CAP_MAX : DAILY_CAP_MAX }
    : mode;

const capMax = (name: string): number => (name === "global_monthly_anlas" ? MONTHLY_CAP_MAX : DAILY_CAP_MAX);

/* ---------------------------------------------------------------- V5 总开关 */

/** V5 暂停（立即生效）+ V5 Full 转 Medium（保存生效）。 */
function V5Card() {
  const { data, error, loading } = useAdmin<Settings>("/admin/api/settings");
  const specs = useMemo(
    () => Object.fromEntries(Object.entries(QUOTA_SPECS).filter(([k]) => k === "v5_medium_mode")),
    [],
  );
  const form = useForm(specs, data as unknown as Record<string, unknown> | null);
  const [saving, setSaving] = useState(false);
  if (!data) return <CardPending title="V5 总开关" loading={loading} error={error} />;
  const save = async () => {
    setSaving(true);
    const ok = await act("PUT", "/admin/api/settings", form.changes(), "模型转换已保存");
    setSaving(false);
    if (ok) form.commit();
  };
  const setPaused = (on: boolean) =>
    act("PUT", "/admin/api/settings", { v5_paused: on }, on ? "V5 已暂停" : "V5 已恢复");
  return (
    <Card title="V5 总开关" sub="暂停立即生效；转换保存后生效">
      <SwitchRow
        title="暂停 V5"
        desc={
          data.v5_paused
            ? "已暂停：V5 请求直接拒（不扣费），首页和客户端都显示暂停"
            : "V5 照常开放，受「每日额度」和访问策略管"
        }
        checked={data.v5_paused}
        onChange={(v) => void setPaused(v)}
      />
      <form class="form-section" {...form.formProps} onSubmit={(e) => e.preventDefault()}>
        <SelectField form={form} name="v5_medium_mode" hint="上游 V5 电量紧张时把 Full 降为 Medium 出。" />
        <SaveBar form={form} onSave={save} saving={saving} label="保存模型转换" />
      </form>
    </Card>
  );
}

/* ---------------------------------------------------------------- 每日额度 */

/** 行式字段：左边标题与说明，右边一个数（额度页就这几项，这样更好对照着读）。 */
function QuotaRow(props: { name: ModeName; value: number; onChange: (v: number) => void }) {
  // 这六个名字在 QUOTA_SPECS 里都是数字规格
  const spec = QUOTA_SPECS[props.name] as NumSpec;
  return (
    <div class="field-row">
      <div class="field-row__label">
        <span class="field-row__name">{spec.label}</span>
        <small class="field-row__hint">{spec.hint}</small>
      </div>
      <div class="inputwrap">
        <input
          class="input"
          type="number"
          min="0"
          max={capMax(props.name)}
          step={spec.kind === "float" ? "any" : 1}
          aria-label={`${spec.label} 限额（0 = 停用）`}
          value={props.value}
          onInput={(e) => props.onChange(Number(e.currentTarget.value))}
        />
        {spec.unit && (
          <span class="inputwrap__unit" aria-hidden="true">
            {spec.unit}
          </span>
        )}
      </div>
    </div>
  );
}

const QUOTA_GROUPS: Array<{ title: string; names: ModeName[] }> = [
  { title: "成员每人", names: ["register_daily_images", "register_daily_v5", "member_daily_anlas"] },
  { title: "全站预算", names: ["global_daily_v5", "global_daily_anlas", "global_monthly_anlas"] },
];

function QuotaCard() {
  const { data, error, loading } = useAdmin<GlobalQuotaModes>("/admin/api/quota-modes");
  if (!data) return <CardPending title="每日额度" loading={loading} error={error} />;
  return <QuotaModesForm data={data} />;
}

/** 10-27 规范化：全局额度就是一个数字（0 = 停用，上限 9999 / 每月池 99999），没有「不限」档。 */
function QuotaModesForm(props: { data: GlobalQuotaModes }) {
  const normalize = (d: GlobalQuotaModes): GlobalQuotaModes => {
    const out = { ...d };
    for (const g of QUOTA_GROUPS) for (const name of g.names) out[name] = boundMode(name, d[name]);
    return out;
  };
  // 草稿 = 显示值 + 它基于的服务端快照签名。服务端来新值时：一致（保存的回声也算）不动；
  // 没在编辑就跟随；正在编辑就保留草稿（自动刷新不许把输入冲掉）。
  const [draft, setDraft] = useState(() => {
    const n = normalize(props.data);
    return { modes: n, sig: JSON.stringify(n) };
  });
  const [saving, setSaving] = useState(false);
  useEffect(() => {
    const n = normalize(props.data);
    const sig = JSON.stringify(n);
    setDraft((d) => {
      if (sig === d.sig) return d;
      return JSON.stringify(d.modes) === d.sig ? { modes: n, sig } : d;
    });
  }, [props.data]);
  const changed = JSON.stringify(draft.modes) !== draft.sig;
  const save = async () => {
    setSaving(true);
    const ok = await act("PUT", "/admin/api/settings", { quota_modes: draft.modes }, "额度已保存并同步");
    setSaving(false);
    if (ok) setDraft({ modes: draft.modes, sig: JSON.stringify(draft.modes) });
  };
  const setMode = (name: ModeName, value: number) => {
    setDraft((d) => ({ ...d, modes: { ...d.modes, [name]: { mode: "limit", value } } }));
  };
  const modes = draft.modes;
  return (
    <Card title="每日额度" sub="0 = 停用；保存后立即同步给全部成员">
      <form onSubmit={(e) => e.preventDefault()}>
        {QUOTA_GROUPS.map((g) => (
          <div class="form-section" key={g.title}>
            <div class="form-section__title">{g.title}</div>
            {g.names.map((name) => {
              const m = modes[name];
              return (
                <QuotaRow
                  key={name}
                  name={name}
                  value={m.mode === "limit" ? m.value : 0}
                  onChange={(v) => setMode(name, v)}
                />
              );
            })}
          </div>
        ))}
        <div class="form-section">
          <SwitchRow
            title="公共 Vibe / 精确参考"
            desc="该开关独立于付费 Anlas 全站日池；关闭时仅 allow_anlas 成员可使用。"
            checked={modes.public_vibe_enabled}
            onChange={(v) => setDraft((d) => ({ ...d, modes: { ...d.modes, public_vibe_enabled: v } }))}
          />
        </div>
        <div class="savebar">
          <Button type="submit" variant="primary" disabled={!changed || saving} onClick={() => void save()}>
            {saving ? "保存中…" : "保存额度模式"}
          </Button>
          {changed && !saving && <span class="savebar__state">有未保存的额度改动</span>}
        </div>
      </form>
    </Card>
  );
}

/* ---------------------------------------------------------------- V5 访问策略 */

type SubjectMember = {
  discord_id: string | null;
  name: string;
  discord: { username: string | null; display_name: string | null } | null;
};

const ACCESS_MODES: Array<{
  id: Settings["v5_access_mode"];
  label: string;
  title: string;
}> = [
  { id: "legacy", label: "兼容旧设置", title: "按旧模型权限与额度规则" },
  { id: "closed", label: "关闭", title: "所有成员均拒绝" },
  { id: "restricted", label: "仅授权主体", title: "只有授权列表里的成员能用 V5" },
  { id: "everyone", label: "全体成员", title: "普通成员、号主、站长都能用（仍受额度与暂停约束）" },
];

/** 一个授权列表：搜索挑人 + 已选 chips（替掉原来难用的原生多选框）。 */
function MemberField(props: {
  title: string;
  members: SubjectMember[];
  selected: string[];
  onChange: (ids: string[]) => void;
}) {
  const options: SearchOption[] = useMemo(() => {
    const out: SearchOption[] = props.members
      .filter((m) => m.discord_id)
      .map((m) => {
        const who = m.discord?.display_name || m.discord?.username || "Discord 成员";
        return { value: m.discord_id as string, label: `${who} · ${m.name}` };
      });
    // 已选但已不在成员列表里的（退群 / 未关联）也要能显示和移除
    for (const id of props.selected) {
      if (!out.some((o) => o.value === id)) out.push({ value: id, label: `${id}（未关联成员）` });
    }
    return out;
  }, [props.members, props.selected]);
  const labelOf = (id: string) => options.find((o) => o.value === id)?.label ?? id;
  return (
    <div class="form-section">
      <div class="form-section__title">{props.title}</div>
      <SearchMultiSelect
        label={`搜索成员加入${props.title}`}
        values={props.selected}
        options={options}
        allLabel="搜索 Discord 成员…"
        onChange={props.onChange}
      />
      {props.selected.length > 0 && (
        <div class="xa-chips">
          {props.selected.map((id) => (
            <span key={id} class="xa-chip">
              {labelOf(id)}
              <button
                type="button"
                aria-label={`从${props.title}移除 ${labelOf(id)}`}
                onClick={() => props.onChange(props.selected.filter((x) => x !== id))}
              >
                <X size={12} aria-hidden="true" />
              </button>
            </span>
          ))}
          <button type="button" class="xa-chips__clear" onClick={() => props.onChange([])}>
            清空
          </button>
        </div>
      )}
    </div>
  );
}

export function AccessPolicyCard() {
  const settings = useAdmin<Settings>("/admin/api/settings");
  const members = useAdmin<{ members: SubjectMember[] }>("/admin/api/members");
  type PolicyDraft = {
    mode: Settings["v5_access_mode"];
    allowed: string[];
    owners: string[];
    admins: string[];
  };
  const fromSettings = (data: Settings): PolicyDraft => ({
    mode: data.v5_access_mode ?? "legacy",
    allowed: data.v5_authorized_discord_ids ?? [],
    owners: data.gateway_owner_discord_ids ?? [],
    admins: data.gateway_admin_discord_ids ?? [],
  });
  const signature = (value: PolicyDraft) => JSON.stringify(value);
  const [draft, setDraft] = useState<PolicyDraft>({ mode: "legacy", allowed: [], owners: [], admins: [] });
  const [baseline, setBaseline] = useState<string | null>(null);
  const baselineRef = useRef<string | null>(null);
  const [externalUpdate, setExternalUpdate] = useState(false);
  const [saving, setSaving] = useState(false);
  const dirty = baseline !== null && signature(draft) !== baseline;
  const dirtyRef = useRef(dirty);
  dirtyRef.current = dirty;
  const savedSignature = useRef<string | null>(null);
  useBusy(dirty || saving);
  useEffect(() => {
    if (!settings.data) return;
    const current = fromSettings(settings.data);
    const currentSignature = signature(current);
    const currentBaseline = baselineRef.current;
    if (currentBaseline === null || currentSignature === savedSignature.current || !dirtyRef.current) {
      setDraft(current);
      setBaseline(currentSignature);
      baselineRef.current = currentSignature;
      setExternalUpdate(false);
      if (currentSignature === savedSignature.current) savedSignature.current = null;
    } else if (currentSignature !== currentBaseline) {
      setExternalUpdate(true);
    }
  }, [settings.data]);
  if (!settings.data || !members.data) {
    return (
      <CardPending
        title="V5 访问策略"
        loading={settings.loading || members.loading}
        error={settings.error ?? members.error}
      />
    );
  }
  const save = async () => {
    setSaving(true);
    const ok = await act(
      "PUT",
      "/admin/api/settings",
      {
        v5_access_mode: draft.mode,
        v5_authorized_discord_ids: draft.allowed,
        gateway_owner_discord_ids: draft.owners,
        gateway_admin_discord_ids: draft.admins,
      },
      "V5 访问策略已保存",
    );
    setSaving(false);
    if (ok) {
      const saved = signature(draft);
      savedSignature.current = saved;
      setBaseline(saved);
      baselineRef.current = saved;
      setExternalUpdate(false);
    }
  };
  const loadLatest = () => {
    if (!settings.data) return;
    const current = fromSettings(settings.data);
    setDraft(current);
    setBaseline(signature(current));
    baselineRef.current = signature(current);
    setExternalUpdate(false);
  };
  const { mode, allowed, owners, admins } = draft;
  const allowedByPolicy = (ids: string[]) =>
    mode === "everyone" ||
    mode === "legacy" ||
    (mode === "restricted" && ids.some((id) => allowed.includes(id)));
  const preview =
    mode === "closed"
      ? "所有成员均拒绝"
      : mode === "restricted"
        ? `普通成员：未列入授权集合则拒绝；号主：${allowedByPolicy(owners) ? "有号主同时列入授权集合，可使用" : "拒绝（身份本身不授权）"}；站长：${allowedByPolicy(admins) ? "有站长同时列入授权集合，可使用" : "拒绝（身份本身不授权）"}`
        : mode === "everyone"
          ? "普通成员、号主、站长均允许（仍受额度、上游状态和紧急暂停约束）"
          : "按旧模型权限与额度规则";
  return (
    <Card title="V5 访问策略" sub="主体身份独立于成员 Key；站长会话仍需后台认证，身份本身也不绕过紧急暂停。">
      <div class="form-section">
        <div class="form-section__title">谁可以使用 V5</div>
        <Seg
          label="谁可以使用 V5"
          value={mode}
          options={ACCESS_MODES}
          onChange={(m) => setDraft((d) => ({ ...d, mode: m }))}
        />
      </div>
      <MemberField
        title="V5 授权主体"
        members={members.data.members}
        selected={allowed}
        onChange={(allowed) => setDraft((d) => ({ ...d, allowed }))}
      />
      <MemberField
        title="号主身份"
        members={members.data.members}
        selected={owners}
        onChange={(owners) => setDraft((d) => ({ ...d, owners }))}
      />
      <MemberField
        title="站长身份"
        members={members.data.members}
        selected={admins}
        onChange={(admins) => setDraft((d) => ({ ...d, admins }))}
      />
      <p class="muted">
        最终效果预览：{preview}。只有 V5 授权列表决定模型访问；身份角色不会隐式获得 V5。派发前会重新检查策略。
      </p>
      {externalUpdate && (
        <p class="notice" role="status">
          服务端策略已变化，你的未保存选择仍保留。需要放弃草稿并查看最新值时，点击“加载最新策略”。
          <button class="button button--secondary" type="button" onClick={loadLatest}>
            加载最新策略
          </button>
        </p>
      )}
      <div class="savebar">
        <button
          class="btn btn--primary"
          type="button"
          disabled={!dirty || saving}
          onClick={() => void save()}
        >
          {saving ? "正在保存…" : "保存访问策略"}
        </button>
      </div>
    </Card>
  );
}

/* ------------------------------------------------- 账号保护与节奏（手动） */

/** 保护字段按用途分组；接口以后加了新字段也能落到「其他」组里，不会丢。 */
const GUARD_GROUPS: Array<{ title: string; names: string[] }> = [
  { title: "出图节奏", names: ["account_daily_cap", "post_jitter_min", "post_jitter_max"] },
  { title: "排队", names: ["queue_per_member", "queue_per_account", "queue_timeout"] },
  { title: "故障冷却", names: ["image_429_cooldown_seconds"] },
];

function GuardCard() {
  const guard = useAdmin<Guard>("/admin/api/guard");
  const runtime = useAdmin<Record<string, number>>("/admin/api/runtime-limits");
  const specs = useMemo(
    () => (guard.data ? { ...RUNTIME_SPECS, ...guardSpecs(guard.data) } : RUNTIME_SPECS),
    [guard.data],
  );
  const server = useMemo(
    () => (guard.data && runtime.data ? { ...runtime.data, ...guard.data.values } : null),
    [guard.data, runtime.data],
  );
  const form = useForm(specs, server);
  const [saving, setSaving] = useState(false);
  if (!guard.data || !runtime.data)
    return (
      <CardPending
        title="账号保护与节奏（手动）"
        loading={guard.loading || runtime.loading}
        error={guard.error ?? runtime.error}
      />
    );
  const save = async () => {
    const ch = form.changes();
    const run: Record<string, unknown> = {};
    const g: Record<string, unknown> = {};
    for (const [k, v] of Object.entries(ch)) (k in RUNTIME_SPECS ? run : g)[k] = v;
    setSaving(true);
    let ok = true;
    if (Object.keys(run).length)
      ok = (await act("PUT", "/admin/api/runtime-limits", run, "运行限制已保存并生效")) !== null && ok;
    if (ok && Object.keys(g).length)
      ok = (await act("PUT", "/admin/api/guard", g, "账号保护已保存，立即生效")) !== null;
    setSaving(false);
    if (ok) form.commit();
  };
  const names = Object.keys(specs);
  const grouped = new Set(GUARD_GROUPS.flatMap((g) => g.names));
  const rest = names.filter((n) => !grouped.has(n));
  return (
    <Card title="账号保护与节奏（手动）" sub="保存即生效">
      <form {...form.formProps} onSubmit={(e) => e.preventDefault()}>
        {GUARD_GROUPS.map((group) => (
          <div class="form-section" key={group.title}>
            <div class="form-section__title">{group.title}</div>
            <div class="form-grid">
              {group.names
                .filter((n) => n in specs)
                .map((n) => (
                  <NumField key={n} form={form} name={n} />
                ))}
            </div>
          </div>
        ))}
        {rest.length > 0 && (
          <div class="form-section">
            <div class="form-section__title">其他</div>
            <div class="form-grid">
              {rest.map((n) => (
                <NumField key={n} form={form} name={n} />
              ))}
            </div>
          </div>
        )}
        <SaveBar form={form} onSave={save} saving={saving} label="保存账号保护" />
      </form>
    </Card>
  );
}

/* ------------------------------------------------- 上游账号现状（只显示） */

/** 保护状态（/admin/api/guard）和电量记录（/admin/api/quota-records）按账号合成一张表。 */
function AccountsCard() {
  const guard = useAdmin<Guard>("/admin/api/guard");
  const records = useAdmin<Records>("/admin/api/quota-records");
  if (!guard.data || !records.data)
    return (
      <CardPending
        title="上游账号现状（只显示）"
        loading={guard.loading || records.loading}
        error={guard.error ?? records.error}
      />
    );
  const rec = new Map(records.data.accounts.map((a) => [a.position, a]));
  const rows = (guard.data.accounts ?? []).map((a) => ({ a, r: rec.get(a.position) }));
  const caps = records.data.manual.account_caps;
  const paid = records.data.manual.paid_v5;
  const now = Date.now();
  return (
    <Card
      title="上游账号现状（只显示）"
      sub="左半是保护与节奏的实时状态，右半是电量与出图量；都不参与任何限额"
    >
      <TableWrap label="上游账号现状">
        <table class="table" data-testid="guard-pace">
          <thead>
            <tr>
              <th>账号</th>
              <th>状态</th>
              <th>今天 出图 / V5</th>
              <th>昨天 出图 / V5</th>
              <th>V5 电量</th>
              <th>Anlas</th>
              <th>周期</th>
              <th>最近 429</th>
              <th>排队</th>
            </tr>
          </thead>
          <tbody>
            {rows.map(({ a, r }) => (
              <tr key={a.position}>
                <td>#{a.position}</td>
                <td>
                  {!a.usable ? (
                    <Badge tone="warn">不可用</Badge>
                  ) : a.full ? (
                    <Badge tone="warn">今日已满</Badge>
                  ) : (
                    <Badge tone="ok">可用</Badge>
                  )}
                  {(a.locked_today ?? 0) > 0 && <span class="muted small"> 锁 {a.locked_today} 次</span>}
                </td>
                <td>
                  {withUnit(a.today, "张")} / {r ? r.today.v5 : "—"}
                </td>
                <td>{r ? `${r.yesterday.images} / ${r.yesterday.v5}` : "—"}</td>
                <td>
                  {dash(r?.v5_percent, "%")}
                  {r?.quiet_rate && (
                    <span
                      class="muted small"
                      title="相邻两次主动读之间该账号没出过 V5 图时，余量的上涨就是纯恢复"
                    >
                      （实测 {r.quiet_rate.rate}）
                    </span>
                  )}
                  <br />
                  <span class="muted small">{dash(r?.v5_recharge_per_day, "%/天")}</span>
                </td>
                <td>{dash(r?.anlas)}</td>
                <td>{dash(a.cycle, " 秒")}</td>
                <td>
                  {agoText(a.last_429, now)}
                  {a.cooling ? `（冷却还剩 ${a.cooling} 秒）` : ""}
                </td>
                <td class="queue-status">
                  {a.waiting ?? 0} 在等
                  {typeof a.queue_cap === "number" ? `，最多 ${a.queue_cap}` : ""}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </TableWrap>
      <div class="form-section" data-testid="quota-records">
        {caps.length === 0 && paid.length === 0 ? (
          <p class="muted small">没有手动临时上限或站付 V5。</p>
        ) : (
          <div class="xa-chips">
            <span class="xa-chips__lead">手动临时</span>
            {caps.map((c) => (
              <span key={`cap-${c.position}`} class="xa-chip">
                账号 {c.position ?? "?"} 每天 {c.cap} 张 · 到 {fullTime(c.until)}
              </span>
            ))}
            {paid.map((p) => (
              <span key={`paid-${p.position}`} class="xa-chip">
                账号 {p.position ?? "?"} 站付 V5 已用 {Math.round(p.spent)} / {Math.round(p.max_anlas)} Anlas
                · 到 {fullTime(p.until)}
              </span>
            ))}
          </div>
        )}
      </div>
    </Card>
  );
}

export default function Limits() {
  return (
    <Sect>
      <SectionHead
        title="额度与保护"
        desc="这一页的数都是手动设的，没有自动算法。V5 总开关与暂停立即生效；额度保存后同步给全部成员。"
      />
      <V5Card />
      <QuotaCard />
      <AccessPolicyCard />
      <GuardCard />
      <AccountsCard />
    </Sect>
  );
}
