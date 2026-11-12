/** 拼车网关：地址 + NAI- Key、允许 Anlas、启停 / 替换 / 删除。份额余量不是官方电池。 */
import { Plus } from "lucide-preact";
import { useId, useState } from "preact/hooks";
import { ApiError, sendJSON } from "../../../../lib/api";
import { Badge, Button, Card, Dialog, Notice } from "../../../../ui";
import { tick, useAdmin } from "../../data";
import { actOrError, ConfirmDialog, LoadError, Loading, useBusy } from "../_kit";
import { Sect, withTone } from "../system/forms";
import {
  anlasText,
  type ShareAccount,
  type SharePage,
  shareHostProblem,
  shareTokenProblem,
  tokenState,
} from "./core";

function ShareDialog(props: {
  mode: "add" | "replace";
  target?: ShareAccount | undefined;
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
      tick.value++;
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
        add ? "添加拼车网关" : `替换拼车网关：#${props.target?.position ?? ""} ${props.target?.token ?? ""}`
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
            {busyNow ? "正在向拼车网关验证…" : add ? "添加" : "替换"}
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

export function ShareUpstream() {
  const { data, error, loading } = useAdmin<SharePage>("/admin/api/share-upstreams");
  const [dialog, setDialog] = useState<{ mode: "add" | "replace"; target?: ShareAccount } | null>(null);
  const [del, setDel] = useState<ShareAccount | null>(null);
  const [toggle, setToggle] = useState<ShareAccount | null>(null);
  const pool = data?.accounts ?? [];
  const poolSize = data?.pool_size ?? pool.length;
  const lastUsable =
    toggle?.admin_enabled === true &&
    poolSize <= 1 &&
    pool.filter((p) => p.admin_enabled && !p.disabled && p.token_id !== toggle.token_id).length === 0;
  const label = (p: ShareAccount) => `#${p.position} ${p.token}`;

  return (
    <Sect>
      <div class="section-head">
        <p>把朋友的拼车网关当额外上游。份额余量来自对方钱包，不是官网电池。导演工具和标签补全仍走官方号。</p>
        <span class="spacer" />
        <Button variant="primary" onClick={() => setDialog({ mode: "add" })}>
          <Plus size={16} aria-hidden="true" />
          添加拼车网关
        </Button>
      </div>
      {loading && !data && <Loading rows={2} />}
      {!data && error && <LoadError error={error} />}
      {data && pool.length === 0 && <Notice>还没有拼车网关。官方账号在「官方账号」页。</Notice>}
      {data && (
        <div class="stack">
          {pool
            .slice()
            .sort(
              (x, y) =>
                Number(y.admin_enabled && !y.disabled) - Number(x.admin_enabled && !x.disabled) ||
                x.position - y.position,
            )
            .map((a) => {
              const st = tokenState(a);
              return (
                <Card key={a.token_id} class="up-account">
                  <header class="token__head">
                    <h2 class="up-account__title">拼车 {a.position}</h2>
                    <span class="token__name mono">{a.token}</span>
                    <Badge {...withTone(st.tone)} dot>
                      {st.text}
                    </Badge>
                    <span class="spacer" />
                    <Button onClick={() => setDialog({ mode: "replace", target: a })}>替换</Button>
                    <Button onClick={() => setToggle(a)}>{a.admin_enabled ? "停用" : "启用"}</Button>
                    <Button disabled={poolSize <= 1} onClick={() => setDel(a)}>
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
                      <dt>允许 Anlas</dt>
                      <dd>{a.allow_anlas ? "是" : "否"}</dd>
                    </div>
                  </dl>
                </Card>
              );
            })}
        </div>
      )}
      {dialog && <ShareDialog mode={dialog.mode} target={dialog.target} onClose={() => setDialog(null)} />}
      {del && (
        <ConfirmDialog
          title="删除拼车网关？"
          confirmText="删除"
          danger
          onClose={() => setDel(null)}
          onConfirm={() =>
            actOrError(
              "DELETE",
              `/admin/api/share-upstreams/${encodeURIComponent(del.token_id)}`,
              undefined,
              "已删除拼车网关",
            )
          }
        >
          <p>从令牌池删除 {label(del)}？它将不再接收新请求，已发出的请求继续完成。</p>
          <Notice tone="warn">删除后如需恢复，要重新粘贴 Key 和地址。</Notice>
        </ConfirmDialog>
      )}
      {toggle && (
        <ConfirmDialog
          title={toggle.admin_enabled ? "停用这条拼车网关？" : "启用这条拼车网关？"}
          confirmText={toggle.admin_enabled ? "停用" : "启用"}
          danger={toggle.admin_enabled}
          onClose={() => setToggle(null)}
          onConfirm={() =>
            actOrError(
              "PUT",
              `/admin/api/upstream-tokens/${encodeURIComponent(toggle.token_id)}/enabled`,
              { enabled: !toggle.admin_enabled },
              toggle.admin_enabled ? "拼车网关已停用" : "拼车网关已启用",
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
