/** 拼车网关：地址 + NAI- Key、允许 Anlas、启停 / 替换 / 删除。份额余量不是官方电池。 */
import { Plus } from "lucide-preact";
import { useId, useState } from "preact/hooks";
import { ApiError, sendJSON } from "../../../../lib/api";
import { Badge, Button, Card, Dialog, Notice } from "../../../../ui";
import { act, refreshAfterWrite, useAdmin } from "../../data";
import { actOrError, ConfirmDialog, LoadError, Loading, useBusy } from "../_kit";
import { NumField, SaveBar, Sect, SwitchRow, useForm, withTone } from "../system/forms";
import { RequestStats } from "./accounts";
import {
  anlasText,
  reading,
  type ShareAccount,
  type SharePage,
  shareHostProblem,
  shareLimitSpecs,
  shareTokenProblem,
  tokenState,
} from "./core";

function ShareDialog(props: {
  mode: "add" | "replace";
  target?: ShareAccount | undefined;
  noun: string;
  onClose: () => void;
}) {
  const add = props.mode === "add";
  const [token, setToken] = useState("");
  const [host, setHost] = useState(add ? "" : (props.target?.image_host ?? ""));
  const [anlas, setAnlas] = useState(false);
  const [err, setErr] = useState("");
  const [busyNow, setBusyNow] = useState(false);
  const id = useId();
  useBusy(true);

  const close = () => {
    if (busyNow) return;
    props.onClose();
  };
  const submit = async () => {
    const tokenErr = shareTokenProblem(token);
    const hostErr = shareHostProblem(host);
    if (tokenErr || hostErr) {
      setErr(tokenErr ?? hostErr ?? "");
      return;
    }
    setErr("");
    setBusyNow(true);
    try {
      const url = add
        ? "/admin/api/share-upstreams"
        : `/admin/api/share-upstreams/${encodeURIComponent(props.target?.token_id ?? "")}`;
      await sendJSON(
        add ? "POST" : "PUT",
        url,
        add
          ? { token: token.trim(), image_host: host.trim(), allow_anlas: anlas }
          : { token: token.trim(), image_host: host.trim() },
      );
      refreshAfterWrite();
      setBusyNow(false);
      props.onClose();
    } catch (e) {
      setBusyNow(false);
      setErr(e instanceof ApiError || e instanceof Error ? e.message : String(e));
    }
  };

  return (
    <Dialog
      open
      title={
        add
          ? `添加${props.noun}`
          : `替换${props.noun}：#${props.target?.position ?? ""} ${props.target?.token ?? ""}`
      }
      onClose={close}
      footer={
        <>
          <Button onClick={close} disabled={busyNow}>
            取消
          </Button>
          <Button
            variant="primary"
            onClick={() => void submit()}
            disabled={busyNow || !token.trim() || !host.trim()}
          >
            {busyNow ? `正在向${props.noun}验证…` : add ? "添加" : "替换"}
          </Button>
        </>
      }
    >
      <form
        class="stack-sm"
        onSubmit={(e) => {
          e.preventDefault();
          void submit();
        }}
      >
        <div class="field">
          <label class="field__label" for={`${id}-h`}>
            拼车网关地址
          </label>
          <input
            id={`${id}-h`}
            class="input mono"
            type="url"
            autoComplete="off"
            spellcheck={false}
            placeholder="https://…"
            value={host}
            disabled={busyNow}
            onInput={(e) => setHost((e.target as HTMLInputElement).value)}
          />
        </div>
        <div class="field">
          <label class="field__label" for={`${id}-t`}>
            拼车网关 Key
          </label>
          <input
            id={`${id}-t`}
            class="input mono"
            type="password"
            autoComplete="off"
            spellcheck={false}
            placeholder="NAI-…"
            value={token}
            disabled={busyNow}
            aria-invalid={err ? "true" : undefined}
            onInput={(e) => setToken((e.target as HTMLInputElement).value)}
          />
          {err && (
            <span class="field__error" role="alert">
              {err}
            </span>
          )}
        </div>
        {add && (
          <label class="check">
            <input
              type="checkbox"
              checked={anlas}
              disabled={busyNow}
              onChange={(e) => setAnlas((e.target as HTMLInputElement).checked)}
            />
            <span>允许消耗 Anlas</span>
          </label>
        )}
      </form>
    </Dialog>
  );
}

function remainingText(a: ShareAccount): string {
  if (a.error) return a.error;
  const parts: string[] = [];
  if (a.usage_percent != null) parts.push(`份额 ${a.usage_percent}%`);
  if (a.anlas != null) parts.push(`Anlas ${anlasText(a.anlas)}`);
  if (a.active === false) parts.push("未生效");
  return parts.length ? parts.join(" · ") : "—";
}

function ShareCard(props: {
  a: ShareAccount;
  limit: { min: number; max: number };
  poolSize: number;
  onReplace: (a: ShareAccount) => void;
  onToggle: (a: ShareAccount) => void;
  onDelete: (a: ShareAccount) => void;
}) {
  const a = props.a;
  const st = tokenState(a);
  const form = useForm(shareLimitSpecs(props.limit), a as unknown as Record<string, unknown>);
  const [saving, setSaving] = useState(false);
  const [partial, setPartial] = useState(false);
  const save = async () => {
    if (Number(form.raw("post_wait_max")) < Number(form.raw("post_wait_min"))) return;
    const ch = form.changes();
    setSaving(true);
    setPartial(false);
    let ok = true;
    let v5Saved = false;
    if ("v5_daily_limit" in ch)
      v5Saved = ok =
        (await act(
          "PUT",
          `/admin/api/upstream-tokens/${encodeURIComponent(a.token_id)}/v5-limit`,
          { v5_daily_limit: ch.v5_daily_limit },
          "V5 日限已保存",
        )) !== null;
    if (ok && Object.keys(ch).some((name) => name !== "v5_daily_limit")) {
      const names = ["allow_anlas", "daily_anlas", "rpm", "concurrency", "post_wait_min", "post_wait_max"];
      const values = form.values();
      const settings = Object.fromEntries(
        names.map((name) => [name, values[name] ?? a[name as keyof ShareAccount]]),
      );
      ok =
        (await act(
          "PUT",
          `/admin/api/share-upstreams/${encodeURIComponent(a.token_id)}/settings`,
          settings,
          "拼车设置已保存",
        )) !== null;
    }
    if (!ok && v5Saved) setPartial(true);
    setSaving(false);
    if (ok) form.commit();
  };
  return (
    <Card class="up-account">
      <header class="token__head">
        <h2 class="up-account__title">
          {a.donated ? "捐赠" : "拼车"} {a.position}
        </h2>
        <span class="token__name mono">{a.token}</span>
        {a.donated && <Badge tone="info">赞助席捐赠</Badge>}
        <Badge {...withTone(st.tone)} dot>
          {st.text}
        </Badge>
        <span class="spacer" />
        <Button onClick={() => props.onReplace(a)}>替换</Button>
        <Button onClick={() => props.onToggle(a)}>{a.admin_enabled ? "停用" : "启用"}</Button>
        <Button disabled={props.poolSize <= 1} onClick={() => props.onDelete(a)}>
          删除
        </Button>
      </header>
      <dl class="up-facts">
        <div>
          <dt>地址</dt>
          <dd class="mono">{a.image_host ?? "—"}</dd>
        </div>
        <div>
          <dt>份额余量</dt>
          <dd>{remainingText(a)}</dd>
        </div>
        <div>
          <dt>今日 Anlas（含在途预留）</dt>
          <dd>
            {reading(a.anlas_used_today)} / {reading(a.daily_anlas)}
          </dd>
        </div>
        <div>
          <dt>今天出图</dt>
          <dd>
            {reading(a.images_today)} 张 · 免费 V5 {reading(a.v5_used)} / {reading(a.v5_daily_limit)}
          </dd>
        </div>
      </dl>
      {a.w1h && a.w24h ? <RequestStats a={a} /> : null}
      <form class="token__form share-settings" {...form.formProps} onSubmit={(e) => e.preventDefault()}>
        {partial && (
          <Notice tone="warn">免费 V5 日限已保存，其他拼车设置的结果未确认。请刷新核对后再重试。</Notice>
        )}
        <NumField form={form} name="v5_daily_limit" disabled={saving} />
        <SwitchRow
          title="允许消耗 Anlas"
          checked={form.raw("allow_anlas") === true}
          onChange={(value) => form.set("allow_anlas", value)}
          disabled={saving}
        />
        <NumField form={form} name="daily_anlas" disabled={saving} />
        <NumField form={form} name="rpm" disabled={saving} />
        <NumField form={form} name="concurrency" disabled={saving} />
        <NumField form={form} name="post_wait_min" disabled={saving} />
        <NumField form={form} name="post_wait_max" disabled={saving} />
        {Number(form.raw("post_wait_max")) < Number(form.raw("post_wait_min")) && (
          <Notice tone="danger">等待时间上限不能小于下限</Notice>
        )}
        <SaveBar
          form={form}
          onSave={save}
          saving={saving || Number(form.raw("post_wait_max")) < Number(form.raw("post_wait_min"))}
        />
      </form>
    </Card>
  );
}

/** 拼车网关与赞助席捐赠账号：同一个共享池、同一套操作，列表里带「捐赠」标记。 */
export function ShareUpstream() {
  const { data, error, loading } = useAdmin<SharePage>("/admin/api/share-upstreams");
  const [dialog, setDialog] = useState<{ mode: "add" | "replace"; target?: ShareAccount } | null>(null);
  const [del, setDel] = useState<ShareAccount | null>(null);
  const [toggle, setToggle] = useState<ShareAccount | null>(null);
  const pool = data?.accounts ?? [];
  const nounOf = (a?: ShareAccount | null) => (a?.donated ? "捐赠网关" : "拼车网关");
  const poolSize = data?.pool_size ?? pool.length;
  const lastUsable =
    toggle?.admin_enabled === true &&
    poolSize <= 1 &&
    pool.filter((p) => p.admin_enabled && !p.disabled && p.token_id !== toggle.token_id).length === 0;
  const label = (p: ShareAccount) => `#${p.position} ${p.token}`;

  return (
    <Sect>
      <div class="section-head">
        <p>
          把朋友的拼车网关当额外上游；赞助席捐赠的 Key
          也在这里，一起进全站共享池。份额余量来自对方钱包，不是官网电池。
        </p>
        <span class="spacer" />
        <Button variant="primary" onClick={() => setDialog({ mode: "add" })}>
          <Plus size={16} aria-hidden="true" />
          添加拼车网关
        </Button>
      </div>
      {loading && !data && <Loading rows={2} />}
      {!data && error && <LoadError error={error} />}
      {data && pool.length === 0 && (
        <Notice>还没有拼车网关。捐赠账号在成员详情开通赞助席并绑定后自动出现。</Notice>
      )}
      {data && (
        <div class="stack">
          {pool
            .slice()
            .sort(
              (x, y) =>
                Number(y.admin_enabled && !y.disabled) - Number(x.admin_enabled && !x.disabled) ||
                x.position - y.position,
            )
            .map((a) => (
              <ShareCard
                key={a.token_id}
                a={a}
                limit={data.fields.v5_daily_limit}
                poolSize={poolSize}
                onReplace={(t) => setDialog({ mode: "replace", target: t })}
                onToggle={setToggle}
                onDelete={setDel}
              />
            ))}
        </div>
      )}
      {dialog && (
        <ShareDialog
          mode={dialog.mode}
          target={dialog.target}
          noun={nounOf(dialog.target)}
          onClose={() => setDialog(null)}
        />
      )}
      {del && (
        <ConfirmDialog
          title={`删除${nounOf(del)}？`}
          confirmText="删除"
          danger
          onClose={() => setDel(null)}
          onConfirm={() =>
            actOrError(
              "DELETE",
              `/admin/api/share-upstreams/${encodeURIComponent(del.token_id)}`,
              undefined,
              `已删除${nounOf(del)}`,
            )
          }
        >
          <p>从令牌池删除 {label(del)}？它将不再接收新请求，已发出的请求继续完成。</p>
          <Notice tone="warn">删除后如需恢复，要重新粘贴 Key 和地址。</Notice>
        </ConfirmDialog>
      )}
      {toggle && (
        <ConfirmDialog
          title={toggle.admin_enabled ? `停用这条${nounOf(toggle)}？` : `启用这条${nounOf(toggle)}？`}
          confirmText={toggle.admin_enabled ? "停用" : "启用"}
          danger={toggle.admin_enabled}
          onClose={() => setToggle(null)}
          onConfirm={() =>
            actOrError(
              "PUT",
              `/admin/api/upstream-tokens/${encodeURIComponent(toggle.token_id)}/enabled`,
              { enabled: !toggle.admin_enabled },
              toggle.admin_enabled ? `${nounOf(toggle)}已停用` : `${nounOf(toggle)}已启用`,
            )
          }
        >
          {toggle.admin_enabled ? (
            lastUsable ? (
              <Notice tone="danger">这是最后一把可用的上游：停用后全站将无法出图，直到重新启用。</Notice>
            ) : (
              <p>停用 {label(toggle)} 后，它不再接收新请求；已发出的请求继续完成。</p>
            )
          ) : (
            <p>启用 {label(toggle)} 后，它重新接收请求。</p>
          )}
        </ConfirmDialog>
      )}
    </Sect>
  );
}
