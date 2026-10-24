/** 额度与保护：每个数都是站长手动设的（10-23 起没有自动额度算法）；最后一张「记录」只显示。 */
import { useMemo, useState } from "preact/hooks";
import { fullTime } from "../../../lib/format";
import { Card } from "../../../ui";
import { act, useAdmin } from "../Admin";
import { CardPending, SectionHead, TableWrap } from "./_kit";
import {
  agoText,
  dash,
  type Guard,
  guardSpecs,
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

/** V5 暂停时这两项不起作用：输入框变灰并注明 */
const V5_PAUSED_FIELDS: Record<string, string> = {
  register_daily_v5: "V5 已暂停",
  global_daily_v5: "V5 已暂停",
};

function QuotaCard() {
  const { data, error, loading } = useAdmin<Settings>("/admin/api/settings");
  if (!data) return <CardPending title="额度" loading={loading} error={error} />;
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
      <FormCard
        title="成员统一额度"
        sub="保存后生效。图片额度同步至采用统一额度的 Key；独立额度保留。"
        url="/admin/api/settings"
        specs={Object.fromEntries(
          Object.entries(QUOTA_SPECS).filter(([k]) => k !== "v5_medium_mode" && !k.startsWith("global_")),
        )}
        server={data as unknown as Record<string, unknown>}
        done="额度已保存并同步"
        saveLabel="保存额度"
        {...(data.v5_paused ? { disabled: V5_PAUSED_FIELDS } : {})}
      />
      <FormCard
        title="全站预算"
        url="/admin/api/settings"
        specs={Object.fromEntries(Object.entries(QUOTA_SPECS).filter(([k]) => k.startsWith("global_")))}
        server={data as unknown as Record<string, unknown>}
        done="全站预算已保存"
        saveLabel="保存全站预算"
        {...(data.v5_paused ? { disabled: V5_PAUSED_FIELDS } : {})}
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
                  <td>{a.today} 张</td>
                  <td>{dash(a.cycle, " 秒")}</td>
                  <td>
                    {agoText(a.last_429, now)}
                    {a.cooling ? `（冷却还剩 ${a.cooling} 秒）` : ""}
                  </td>
                  <td>{a.locked_today ?? 0} 次</td>
                  <td>
                    {a.waiting ?? 0} 张在等{typeof a.queue_cap === "number" ? `，最多 ${a.queue_cap}` : ""}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </TableWrap>
        <p class="muted small">
          全站排队上限：{typeof data.queue_cap === "number" ? `${data.queue_cap} 张` : "不限"}
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
            手动临时：账号 {c.position ?? "?"} 每天 {c.cap} 张，到 {fullTime(c.until)}
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
      <SectionHead title="额度与保护" desc="这一页的数都是手动设的，没有自动算法；「记录」只显示" />
      <QuotaCard />
      <GuardCard />
      <RecordsCard />
    </Sect>
  );
}
