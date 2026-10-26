/** 额度与保护：每个数都是站长手动设的（10-23 起没有自动额度算法）；最后一张「记录」只显示。 */
import { useEffect, useMemo, useRef, useState } from "preact/hooks";
import { fullTime, withUnit } from "../../../lib/format";
import { Button, Card } from "../../../ui";
import { act, useAdmin } from "../Admin";
import { CardPending, SectionHead, TableWrap, useBusy } from "./_kit";
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
import type { FieldSpec } from "./system/core";
import { NumField, SaveBar, Sect, SelectField, SwitchRow, useForm } from "./system/forms";

/** 一张手动设置卡：字段、保存。 */
function FormCard(props: {
  title: string;
  sub?: string;
  url: string;
  specs: Record<string, FieldSpec>;
  server: Record<string, unknown> | null;
  done: string;
  saveLabel: string;
  /** 暂时不能改的字段 → 说明（输入框变灰，说明顶替原来的提示） */
  disabled?: Record<string, string>;
}) {
  const form = useForm(props.specs, props.server);
  const [saving, setSaving] = useState(false);
  const save = async () => {
    setSaving(true);
    const ok = await act("PUT", props.url, form.changes(), props.done);
    setSaving(false);
    if (ok) form.commit();
  };
  return (
    <Card title={props.title} {...(props.sub ? { sub: props.sub } : {})}>
      <form {...form.formProps} onSubmit={(e) => e.preventDefault()}>
        <div class="form-grid">
          {Object.keys(props.specs).map((n) =>
            props.specs[n]?.kind === "select" ? (
              <SelectField key={n} form={form} name={n} />
            ) : (
              <NumField
                key={n}
                form={form}
                name={n}
                {...(props.disabled?.[n] ? { disabled: true, hint: props.disabled[n] } : {})}
              />
            ),
          )}
        </div>
        <SaveBar form={form} onSave={save} saving={saving} label={props.saveLabel} />
      </form>
    </Card>
  );
}

function QuotaCard() {
  const { data, error, loading } = useAdmin<Settings>("/admin/api/settings");
  const {
    data: modes,
    error: modesError,
    loading: modesLoading,
  } = useAdmin<GlobalQuotaModes>("/admin/api/quota-modes");
  if (!data || !modes)
    return <CardPending title="额度" loading={loading || modesLoading} error={error || modesError} />;
  const setPaused = (on: boolean) =>
    act("PUT", "/admin/api/settings", { v5_paused: on }, on ? "V5 已暂停" : "V5 已恢复");
  return (
    <>
      <Card title="V5 暂停（手动）">
        <SwitchRow
          title="暂停 V5"
          desc={
            data.v5_paused
              ? "已暂停：V5 请求直接拒（不扣费），首页和客户端都显示暂停"
              : "V5 照常开放，受下面的每人 / 全站上限管"
          }
          checked={data.v5_paused}
          onChange={(v) => void setPaused(v)}
        />
      </Card>
      <AccessPolicyCard />
      <QuotaModesCard
        title="成员统一额度"
        data={modes}
        names={["register_daily_images", "register_daily_v5", "member_daily_anlas"]}
        done="额度已保存并同步"
      />
      <QuotaModesCard
        title="全站预算"
        data={modes}
        names={["global_daily_v5", "global_daily_anlas", "global_monthly_anlas"]}
        done="全站预算已保存"
      />
      <FormCard
        title="模型转换"
        url="/admin/api/settings"
        specs={Object.fromEntries(Object.entries(QUOTA_SPECS).filter(([k]) => k === "v5_medium_mode"))}
        server={data as unknown as Record<string, unknown>}
        done="模型转换已保存"
        saveLabel="保存模型转换"
      />
    </>
  );
}

const MODE_LABELS: Record<
  keyof Omit<GlobalQuotaModes, "public_vibe_enabled">,
  { label: string; unit: string }
> = {
  register_daily_images: { label: "V4.5 每人每天", unit: "张" },
  register_daily_v5: { label: "V5 每人每天", unit: "张" },
  global_daily_v5: { label: "V5 全站每天", unit: "张" },
  global_daily_anlas: { label: "Anlas 全站每天付费池", unit: "Anlas" },
  member_daily_anlas: { label: "Anlas 每人每天", unit: "Anlas" },
  global_monthly_anlas: { label: "Anlas 全站每月", unit: "Anlas" },
};

/** 迁移前的旧数据可能是 unlimited：显示与编辑都折算成上限值（9999 / 每月池 99999）。 */
const boundMode = (name: keyof typeof MODE_LABELS, mode: GlobalQuotaModes[keyof typeof MODE_LABELS]) =>
  mode.mode === "unlimited"
    ? { mode: "limit" as const, value: name === "global_monthly_anlas" ? MONTHLY_CAP_MAX : DAILY_CAP_MAX }
    : mode;

function QuotaModesCard(props: {
  title: string;
  data: GlobalQuotaModes;
  names: Array<keyof typeof MODE_LABELS>;
  done: string;
}) {
  // 10-27 规范化：全局额度就是一个数字（0 = 停用，上限 9999 / 每月池 99999），没有「不限」档
  const normalize = (d: GlobalQuotaModes): GlobalQuotaModes => {
    const out = { ...d };
    for (const name of props.names) out[name] = boundMode(name, d[name]);
    return out;
  };
  const [modes, setModes] = useState(() => normalize(props.data));
  const [saving, setSaving] = useState(false);
  useEffect(() => setModes(normalize(props.data)), [props.data]);
  const changed = JSON.stringify(modes) !== JSON.stringify(normalize(props.data));
  const save = async () => {
    setSaving(true);
    const ok = await act("PUT", "/admin/api/settings", { quota_modes: modes }, props.done);
    setSaving(false);
    if (ok) setModes(modes);
  };
  const setMode = (name: keyof typeof MODE_LABELS, value: GlobalQuotaModes[keyof typeof MODE_LABELS]) => {
    setModes((current) => ({ ...current, [name]: value }));
  };
  return (
    <Card title={props.title}>
      <div class="form-grid">
        {props.names.map((name) => {
          const mode = modes[name];
          const spec = MODE_LABELS[name];
          const max = name === "global_monthly_anlas" ? MONTHLY_CAP_MAX : DAILY_CAP_MAX;
          return (
            <div class="field" key={name}>
              <span>{spec.label}</span>
              <div class="field__control">
                <input
                  class="input"
                  type="number"
                  min="0"
                  max={max}
                  step={name === "global_monthly_anlas" ? "any" : 1}
                  aria-label={`${spec.label} 限额（0 = 停用）`}
                  value={mode.mode === "limit" ? mode.value : 0}
                  onInput={(e) => setMode(name, { mode: "limit", value: Number(e.currentTarget.value) })}
                />
                <small>
                  {spec.unit}；0 = 停用，上限 {max.toLocaleString("en-US")}
                </small>
              </div>
            </div>
          );
        })}
        {props.names.includes("global_daily_anlas") && (
          <SwitchRow
            title="公共 Vibe / 精确参考"
            desc="该开关独立于付费 Anlas 全站日池；关闭时仅 allow_anlas 成员可使用。"
            checked={modes.public_vibe_enabled}
            onChange={(v) => setModes((current) => ({ ...current, public_vibe_enabled: v }))}
          />
        )}
      </div>
      <div class="savebar">
        <Button type="button" variant="primary" disabled={!changed || saving} onClick={() => void save()}>
          {saving ? "保存中…" : "保存额度模式"}
        </Button>
      </div>
    </Card>
  );
}

type SubjectMember = {
  discord_id: string | null;
  name: string;
  discord: { username: string | null; display_name: string | null } | null;
};

function MemberPicker(props: {
  title: string;
  members: SubjectMember[];
  selected: string[];
  onChange: (ids: string[]) => void;
}) {
  const selectedSet = new Set(props.selected);
  const mapped = props.members.filter((m) => m.discord_id).map((m) => m.discord_id as string);
  const ids = [...new Set([...mapped, ...props.selected])];
  const label = (id: string) => {
    const member = props.members.find((m) => m.discord_id === id);
    if (!member) return `${id}（未关联成员）`;
    const who = member.discord?.display_name || member.discord?.username || "Discord 成员";
    return `${who} · ${member.name} · ${id}`;
  };
  return (
    <label class="field">
      <span>{props.title}</span>
      <select
        multiple
        size={Math.min(Math.max(ids.length, 4), 10)}
        ref={(el) => {
          if (!el) return;
          for (const option of el.options) option.selected = selectedSet.has(option.value);
        }}
        onChange={(e) => {
          const select = e.currentTarget as HTMLSelectElement;
          props.onChange(Array.from(select.selectedOptions, (option) => option.value));
        }}
      >
        {ids.map((id) => (
          <option key={id} value={id}>
            {label(id)}
            {selectedSet.has(id) ? " ✓" : ""}
          </option>
        ))}
      </select>
      <small>
        按住 Shift 或 Command 可多选；已选：
        {props.selected.length ? props.selected.map(label).join("、") : "无"}
      </small>
    </label>
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
  const mode = draft.mode;
  const allowed = draft.allowed;
  const owners = draft.owners;
  const admins = draft.admins;
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
          ? "普通成员、号主、站长均允许（仍受配额、上游状态和紧急暂停约束）"
          : "按旧模型权限与额度规则";
  return (
    <Card title="V5 访问策略" sub="主体身份独立于成员 Key；站长会话仍需后台认证，身份本身也不绕过紧急暂停。">
      <div class="form-grid">
        <label class="field">
          <span>谁可以使用 V5</span>
          <select
            value={mode}
            onChange={(e) =>
              setDraft((d) => ({
                ...d,
                mode: (e.currentTarget as HTMLSelectElement).value as Settings["v5_access_mode"],
              }))
            }
          >
            <option value="legacy">兼容旧设置</option>
            <option value="closed">关闭</option>
            <option value="restricted">仅授权主体</option>
            <option value="everyone">全体成员</option>
          </select>
        </label>
        <MemberPicker
          title="V5 授权主体"
          members={members.data.members}
          selected={allowed}
          onChange={(allowed) => setDraft((d) => ({ ...d, allowed }))}
        />
        <MemberPicker
          title="号主身份"
          members={members.data.members}
          selected={owners}
          onChange={(owners) => setDraft((d) => ({ ...d, owners }))}
        />
        <MemberPicker
          title="站长身份"
          members={members.data.members}
          selected={admins}
          onChange={(admins) => setDraft((d) => ({ ...d, admins }))}
        />
      </div>
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
      <button class="button" type="button" disabled={!dirty || saving} onClick={() => void save()}>
        {saving ? "正在保存…" : "保存访问策略"}
      </button>
    </Card>
  );
}

/** 账号保护、排队、出图节奏与冷却：两个接口的手动设置合在一张卡里，保存时各自提交。 */
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
  const data = guard.data;
  if (!data || !runtime.data)
    return (
      <CardPending
        title="账号保护、排队与出图节奏（手动）"
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
  const now = Date.now();
  return (
    <>
      <Card title="账号保护、排队与出图节奏（手动）" sub="保存即生效">
        <form {...form.formProps} onSubmit={(e) => e.preventDefault()}>
          <div class="form-grid">
            {Object.keys(specs).map((n) => (
              <NumField key={n} form={form} name={n} />
            ))}
          </div>
          <SaveBar form={form} onSave={save} saving={saving} label="保存账号保护" />
        </form>
      </Card>
      <Card title="每个账号现在（只显示）">
        <TableWrap label="每个账号现在">
          <table class="table" data-testid="guard-pace">
            <thead>
              <tr>
                <th>账号</th>
                <th>今天</th>
                <th>周期</th>
                <th>最近 429</th>
                <th>并发锁（今天）</th>
                <th>排队</th>
              </tr>
            </thead>
            <tbody>
              {(data.accounts ?? []).map((a) => (
                <tr key={a.position}>
                  <td>
                    #{a.position}
                    {!a.usable ? "（不可用）" : a.full ? "（今日已满）" : ""}
                  </td>
                  <td>{withUnit(a.today, "张")}</td>
                  <td>{dash(a.cycle, " 秒")}</td>
                  <td>
                    {agoText(a.last_429, now)}
                    {a.cooling ? `（冷却还剩 ${withUnit(a.cooling, "秒")}）` : ""}
                  </td>
                  <td>{a.locked_today ?? 0} 次</td>
                  <td>
                    {withUnit(a.waiting ?? 0, "张")}在等
                    {typeof a.queue_cap === "number" ? `，最多 ${withUnit(a.queue_cap, "张")}` : ""}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </TableWrap>
        <p class="muted small">
          全站排队上限：{typeof data.queue_cap === "number" ? withUnit(data.queue_cap, "张") : "不限"}
        </p>
      </Card>
    </>
  );
}

function RecordsCard() {
  const { data, error, loading } = useAdmin<Records>("/admin/api/quota-records");
  if (!data) return <CardPending title="记录（只显示）" loading={loading} error={error} />;
  return (
    <Card title="记录（只显示）" sub="上游账号的 V5 电量、Anlas 余额和出图量；不参与任何限额">
      <TableWrap label="上游账号记录">
        <table class="table" data-testid="quota-records">
          <thead>
            <tr>
              <th>账号</th>
              <th>V5 电量</th>
              <th>官方恢复</th>
              <th>Anlas</th>
              <th>今天 出图 / V5</th>
              <th>昨天 出图 / V5</th>
            </tr>
          </thead>
          <tbody>
            {data.accounts.map((a) => (
              <tr key={a.position}>
                <td>#{a.position}</td>
                <td>{dash(a.v5_percent, "%")}</td>
                <td>
                  {dash(a.v5_recharge_per_day, "%/天")}
                  {a.quiet_rate ? (
                    <span class="muted" title="相邻两次主动读之间该账号没出过 V5 图时，余量的上涨就是纯恢复">
                      （实测 {a.quiet_rate.rate}）
                    </span>
                  ) : (
                    ""
                  )}
                </td>
                <td>{dash(a.anlas)}</td>
                <td>
                  {a.today.images} / {a.today.v5}
                </td>
                <td>
                  {a.yesterday.images} / {a.yesterday.v5}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </TableWrap>
      <ul class="muted small">
        {data.manual.account_caps.map((c) => (
          <li key={`cap-${c.position}`}>
            手动临时：账号 {c.position ?? "?"} 每天 {withUnit(c.cap, "张")}，到 {fullTime(c.until)}
          </li>
        ))}
        {data.manual.paid_v5.map((p) => (
          <li key={`paid-${p.position}`}>
            手动站付 V5：账号 {p.position ?? "?"} 已用 {Math.round(p.spent)} / {Math.round(p.max_anlas)}{" "}
            Anlas，到 {fullTime(p.until)}
          </li>
        ))}
      </ul>
    </Card>
  );
}

export default function Limits() {
  return (
    <Sect>
      <SectionHead title="额度与保护" desc="这一页的数都是手动设的，没有自动算法。" />
      <QuotaCard />
      <GuardCard />
      <RecordsCard />
    </Sect>
  );
}
