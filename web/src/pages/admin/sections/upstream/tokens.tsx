/** 上游令牌池：添加 / 替换 / 停用 / 删除 Token，调每把 Token 的 V5 日限（出图并发固定 1）。完整 Token 从不显示。 */
import { Ellipsis, Plus } from "lucide-preact";
import { useId, useState } from "preact/hooks";
import { ApiError, sendJSON } from "../../../../lib/api";
import { usedOf, withUnit } from "../../../../lib/format";
import { Badge, Button, Card, Dialog, Menu, Notice, toast } from "../../../../ui";
import { act, tick } from "../../Admin";
import { actOrError, ConfirmDialog, useBusy } from "../_kit";
import { Kv, NumField, SaveBar, useForm, withTone } from "../system/forms";
import { type PoolToken, TOKEN_SPECS, tokenFormatProblem, tokenState } from "./core";

/* ---------------- 添加 / 替换 Token 的对话框 ---------------- */
function TokenDialog(props: {
  mode: "add" | "replace";
  target?: PoolToken | undefined;
  onClose: () => void;
}) {
  const add = props.mode === "add";
  const [token, setToken] = useState("");
  const [anlas, setAnlas] = useState(false);
  const [err, setErr] = useState("");
  const [busyNow, setBusyNow] = useState(false);
  const id = useId();
  useBusy(true);

  const close = () => {
    if (busyNow) return;
    setToken("");
    props.onClose();
  };
  const submit = async () => {
    const problem = tokenFormatProblem(token);
    if (problem) {
      setErr(problem);
      return;
    }
    setErr("");
    setBusyNow(true);
    try {
      const url = add
        ? "/admin/api/upstream-tokens"
        : `/admin/api/upstream-tokens/${encodeURIComponent(props.target?.token_id ?? "")}`;
      const r = await sendJSON<{ tier: number }>(
        add ? "POST" : "PUT",
        url,
        add ? { token: token.trim(), allow_anlas: anlas } : { token: token.trim() },
      );
      toast(`验证通过（订阅等级 ${r.tier}），${add ? "已加入令牌池" : "已替换"}`);
      tick.value++;
      setBusyNow(false);
      setToken("");
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
          ? "添加上游 Token"
          : `替换上游 Token：#${props.target?.position ?? ""} ${props.target?.token ?? ""}`
      }
      onClose={close}
      footer={
        <>
          <Button onClick={close} disabled={busyNow}>
            取消
          </Button>
          <Button variant="primary" onClick={() => void submit()} disabled={busyNow || !token.trim()}>
            {busyNow ? "正在向 NovelAI 验证…" : add ? "添加" : "替换"}
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
        {!add && (
          <p class="muted small">
            在 NovelAI 重置 Token 后，把新的粘贴到这里；这把 Token 的 V5 日限、启停、并发和今日计数会延续。
          </p>
        )}
        <div class="field">
          <label class="field__label" for={`${id}-t`}>
            NovelAI 持久 API Token
          </label>
          <input
            id={`${id}-t`}
            class="input mono"
            type="password"
            autoComplete="off"
            spellcheck={false}
            placeholder="pst-…"
            value={token}
            disabled={busyNow}
            aria-invalid={err ? "true" : undefined}
            aria-describedby={`${id}-h`}
            onInput={(e) => setToken((e.target as HTMLInputElement).value)}
          />
          <span class="field__hint" id={`${id}-h`}>
            在 NovelAI 的 账号设置 → Get Persistent API Token 里获取。粘贴后不会显示，提交前会先向 NovelAI
            验证。
          </span>
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
            <span>允许这把 Token 消耗 Anlas（付费点数）</span>
          </label>
        )}
      </form>
    </Dialog>
  );
}

/* ---------------- 单把 Token ---------------- */
function TokenCard(props: {
  p: PoolToken;
  only: boolean;
  onReplace: (p: PoolToken) => void;
  onDelete: (p: PoolToken) => void;
  onToggle: (p: PoolToken) => void;
}) {
  const p = props.p;
  const st = tokenState(p);
  const form = useForm(TOKEN_SPECS, p as unknown as Record<string, unknown>);
  const [saving, setSaving] = useState(false);
  const base = `/admin/api/upstream-tokens/${encodeURIComponent(p.token_id)}`;
  const save = async () => {
    const ch = form.changes();
    setSaving(true);
    let ok = true;
    if ("v5_daily_limit" in ch)
      ok =
        (await act("PUT", `${base}/v5-limit`, { v5_daily_limit: ch.v5_daily_limit }, "V5 日限已保存")) !==
        null;
    setSaving(false);
    if (ok) form.commit();
  };
  return (
    <article class="token" aria-label={`上游账号 ${p.position}`}>
      <header class="token__head">
        <span class="token__name mono">
          #{p.position} {p.token}
        </span>
        <Badge {...withTone(st.tone)} dot>
          {st.text}
        </Badge>
        <span class="spacer" />
        <Menu
          button={(b) => (
            <Button variant="ghost" icon aria-label={`账号 ${p.position} 的更多操作`} {...b}>
              <Ellipsis size={18} aria-hidden="true" />
            </Button>
          )}
          items={[
            { label: "替换 Token", onSelect: () => props.onReplace(p) },
            {
              label: p.admin_enabled ? "停用这个账号" : "启用这个账号",
              onSelect: () => props.onToggle(p),
              danger: p.admin_enabled,
            },
            ...(props.only ? [] : [{ label: "删除", onSelect: () => props.onDelete(p), danger: true }]),
          ]}
        />
      </header>
      <Kv
        rows={[
          ["今日生成", `${withUnit(p.images_today, "张")} · 失败 ${p.fails} 次`],
          ["免费 V5", `今日已用 ${usedOf(p.v5_used, p.v5_daily_limit)}`],
          ["在途", withUnit(p.image_active, "张")],
          ["Anlas", p.allow_anlas ? "允许消耗" : "禁止消耗"],
        ]}
      />
      <form class="token__form" {...form.formProps} onSubmit={(e) => e.preventDefault()}>
        <div class="form-grid">
          <NumField form={form} name="v5_daily_limit" />
        </div>
        <SaveBar form={form} onSave={save} saving={saving} />
      </form>
    </article>
  );
}

export default function TokensCard(props: { pool: PoolToken[]; configured: boolean }) {
  const [dialog, setDialog] = useState<{ mode: "add" | "replace"; target?: PoolToken } | null>(null);
  const [del, setDel] = useState<PoolToken | null>(null);
  const [toggle, setToggle] = useState<PoolToken | null>(null);

  const usable = props.pool.filter((p) => p.admin_enabled && !p.disabled);
  const lastUsable =
    toggle?.admin_enabled === true && usable.filter((p) => p.token_id !== toggle.token_id).length === 0;
  const label = (p: PoolToken) => `#${p.position} ${p.token}`;

  return (
    <Card
      title="上游令牌池"
      sub="成员的出图请求会在这些 NovelAI 账号之间分配。也可以在服务器的 .env 里用 NAI_TOKENS 配置。"
      actions={
        <Button variant="primary" onClick={() => setDialog({ mode: "add" })}>
          <Plus size={16} aria-hidden="true" />
          添加 Token
        </Button>
      }
    >
      {!props.configured && (
        <Notice tone="danger">
          还没有配置上游 Token，现在所有出图请求都会失败。请点右上角「添加 Token」。
        </Notice>
      )}
      <div class="token-grid">
        {props.pool.map((p) => (
          <TokenCard
            key={p.token_id}
            p={p}
            only={props.pool.length <= 1}
            onReplace={(t) => setDialog({ mode: "replace", target: t })}
            onDelete={setDel}
            onToggle={setToggle}
          />
        ))}
      </div>

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
    </Card>
  );
}
