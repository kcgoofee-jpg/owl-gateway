/** 成员页的各个小对话框：展示新 Key、绑定 Discord、发券、加标签、黑名单。 */
import type { ComponentChildren } from "preact";
import { useEffect, useId, useState } from "preact/hooks";
import { getJSON } from "../../../../lib/api";
import { fullTime, rel, scopeText } from "../../../../lib/format";
import { Badge, Button, CopyField, copyText, Dialog, Field, Notice, Skeleton } from "../../../../ui";
import { Avatar, ConfirmDialog, TableWrap, useBusy } from "../_kit";
import { addBan, addTag, call, giveCoupon, removeBan, resetDailyQuota } from "./actions";
import { scopeTextOf } from "./logic";
import { Empty, ResetScopeChecks, useDebounced, usePageData } from "./shared";
import type { Ban, Coupon } from "./types";
import { isDiscordId, parseNum } from "./validate";

/* ---------------- 通用：带表单的对话框 ---------------- */
function FormDialog(props: {
  title: string;
  submitText: string;
  onClose: () => void;
  /** 返回字符串 = 失败原因（显示在框里）；返回 undefined = 成功并关闭 */
  onSubmit: () => Promise<string | undefined> | string | undefined;
  children: ComponentChildren;
  wide?: boolean;
}) {
  useBusy();
  const id = useId();
  const [pending, setPending] = useState(false);
  const [err, setErr] = useState("");
  return (
    <Dialog
      open
      wide={props.wide ?? false}
      title={props.title}
      onClose={props.onClose}
      footer={
        <>
          <Button onClick={props.onClose}>取消</Button>
          <Button variant="primary" type="submit" form={id} disabled={pending}>
            {pending ? "处理中…" : props.submitText}
          </Button>
        </>
      }
    >
      <form
        id={id}
        class="xa-form"
        noValidate
        onSubmit={async (e) => {
          e.preventDefault();
          if (pending) return;
          setPending(true);
          setErr("");
          try {
            const r = await props.onSubmit();
            if (r === undefined) props.onClose();
            else setErr(r);
          } finally {
            setPending(false);
          }
        }}
      >
        {props.children}
        {err && (
          <p class="xa-formerr" role="alert">
            {err}
          </p>
        )}
      </form>
    </Dialog>
  );
}

const fieldErr = (e: string) =>
  e ? (
    <span class="field__err" role="alert">
      {e}
    </span>
  ) : undefined;

/* ---------------- 展示新 Key / 邀请消息 ---------------- */
export function KeyRevealDialog(props: {
  title: string;
  intro: string;
  token: string;
  /** 邀请 Key：发给对方的整段消息 */
  message?: string;
  onClose: () => void;
}) {
  useBusy();
  const [copied, setCopied] = useState("");
  const copy = async (what: string, text: string) => {
    const ok = await copyText(text);
    setCopied(ok ? what : "复制失败，请手动选中文字复制");
    setTimeout(() => setCopied(""), 2500);
  };
  return (
    <Dialog
      open
      title={props.title}
      onClose={props.onClose}
      footer={
        <Button variant="primary" onClick={props.onClose}>
          完成
        </Button>
      }
    >
      <div class="xa-form">
        <p class="small muted">{props.intro}</p>
        {props.message ? (
          <>
            <Field label="发给对方的完整消息">
              <textarea class="textarea mb-msg" readOnly rows={11} value={props.message} />
            </Field>
            <div class="row">
              <Button onClick={() => copy("已复制 Key", props.token)}>只复制 Key</Button>
              <Button variant="primary" onClick={() => copy("已复制整段消息", props.message ?? "")}>
                复制整段消息
              </Button>
            </div>
          </>
        ) : (
          <CopyField value={props.token} label="Key" wrap />
        )}
        <p class="small" role="status" aria-live="polite">
          {copied}
        </p>
      </div>
    </Dialog>
  );
}

export function CredentialDialog(props: {
  keyId: number;
  name: string;
  expiresAt: number | null;
  onClose: () => void;
  onDone: (text: string) => void;
}) {
  const [date, setDate] = useState(
    props.expiresAt ? new Date(props.expiresAt * 1000 + 8 * 3600000).toISOString().slice(0, 10) : "",
  );
  return (
    <FormDialog
      title={`${props.name} · 到期日期`}
      submitText="保存"
      onClose={props.onClose}
      onSubmit={async () => {
        const expires = date ? Date.parse(`${date}T23:59:59+08:00`) / 1000 : null;
        if (expires !== null && !Number.isFinite(expires)) return "到期日期无效";
        const result = await call("PATCH", `/admin/api/keys/${props.keyId}`, { expires_at: expires });
        if (!result.ok) return result.error;
        props.onDone("到期日期已保存");
      }}
    >
      <Field label="到期日期">
        <input
          class="input"
          type="date"
          value={date}
          onInput={(event) => setDate(event.currentTarget.value)}
        />
      </Field>
      <Button size="sm" onClick={() => setDate("")}>
        设为永久
      </Button>
    </FormDialog>
  );
}

/* ---------------- 重置今日额度 ---------------- */
export function ResetQuotaDialog(props: {
  keyId: number;
  name: string;
  onClose: () => void;
  onDone: (text: string) => void;
}) {
  const [scope, setScope] = useState({ v5: false, anlas: false });
  return (
    <ConfirmDialog
      title={`重置「${props.name}」今日额度`}
      confirmText="重置"
      onClose={props.onClose}
      onConfirm={async () => {
        const r = await resetDailyQuota(props.keyId, scope);
        if (!r.ok) return r.error;
        props.onDone(`已重置「${props.name}」今日的 ${scopeTextOf(scope)} 用量`);
        return undefined;
      }}
    >
      <p>默认只重置今天的 V4.5 用量；V5 和 Anlas 要用的话在下面勾上。图片张数、请求数及请求日志不会删除。</p>
      <ResetScopeChecks value={scope} onChange={setScope} />
    </ConfirmDialog>
  );
}

/* ---------------- 发重置券 ---------------- */
function couponState(c: Coupon, now: number): { text: string; tone?: "ok" | "warn" } {
  if (c.used_at) return { text: `已使用 · ${rel(c.used_at, now)}` };
  if (c.expires_at < now) return { text: "已过期" };
  return { text: `有效至 ${fullTime(c.expires_at)}`, tone: "ok" };
}

export function CouponDialog(props: {
  keyId: number;
  name: string;
  onClose: () => void;
  onDone: (text: string) => void;
}) {
  const [days, setDays] = useState("7");
  const [note, setNote] = useState("站长送的");
  const [scope, setScope] = useState({ v5: false, anlas: false });
  const [err, setErr] = useState("");
  const [list, setList] = useState<Coupon[] | null>(null);
  useEffect(() => {
    getJSON<{ coupons: Coupon[] }>(`/admin/api/keys/${props.keyId}/coupons`)
      .then((r) => setList(r.coupons))
      .catch(() => setList([]));
  }, [props.keyId]);
  const now = Date.now() / 1000;
  return (
    <FormDialog
      title={`发重置券：${props.name}`}
      submitText="发出"
      onClose={props.onClose}
      onSubmit={async () => {
        const d = parseNum(days, "有效期", { min: 1, max: 30 });
        if (!d.ok) {
          setErr(d.error);
          return "请先修正标红的内容";
        }
        setErr("");
        const r = await giveCoupon(props.keyId, d.value, note.trim(), scope);
        if (!r.ok) return r.error;
        props.onDone(
          `已给「${props.name}」发了一张重置券（${scopeTextOf(scope)}），${d.value} 天内有效，成员在首页就能看到`,
        );
        return undefined;
      }}
    >
      <p class="small muted">
        成员在首页自己点一下，就能重置当天的额度；一张券用一次就作废。默认只重置 V4.5。
      </p>
      <ResetScopeChecks value={scope} onChange={setScope} />
      <div class="xa-form__grid">
        <Field label="有效天数" hint={fieldErr(err) ?? "1～30 天"}>
          <input
            class="input"
            type="number"
            inputMode="numeric"
            min={1}
            max={30}
            value={days}
            aria-invalid={err ? true : undefined}
            onInput={(e) => setDays(e.currentTarget.value)}
          />
        </Field>
        <Field label="备注（成员能看到）" hint="最多 200 字">
          <input class="input" maxLength={200} value={note} onInput={(e) => setNote(e.currentTarget.value)} />
        </Field>
      </div>
      <div>
        <h3 class="mb-h4">已发出的券</h3>
        {list === null ? (
          <Skeleton h="36px" />
        ) : list.length === 0 ? (
          <p class="small muted">还没有给这位成员发过券</p>
        ) : (
          <ul class="mb-plain">
            {list.map((c) => {
              const s = couponState(c, now);
              return (
                <li key={c.id}>
                  <Badge {...(s.tone ? { tone: s.tone } : {})}>{s.text}</Badge>
                  <span class="small">
                    {c.note || "重置券"}（{scopeText(c.scope)}）
                  </span>
                  <span class="small muted">
                    {c.by ? `${c.by} · ` : ""}
                    {rel(c.created_at, now)}
                  </span>
                </li>
              );
            })}
          </ul>
        )}
      </div>
    </FormDialog>
  );
}

/* ---------------- 加标签 ---------------- */
export function TagDialog(props: {
  keyId: number;
  name: string;
  onClose: () => void;
  onDone: (text: string) => void;
}) {
  const [tag, setTag] = useState("");
  const [note, setNote] = useState("");
  const [err, setErr] = useState("");
  return (
    <FormDialog
      title={`添加标签：${props.name}`}
      submitText="添加"
      onClose={props.onClose}
      onSubmit={async () => {
        const t = tag.trim();
        if (!t) {
          setErr("标签名不能为空");
          return "请先修正标红的内容";
        }
        setErr("");
        const r = await addTag(props.keyId, t, note.trim());
        if (!r.ok) return r.error;
        props.onDone(`已标记：${t}`);
        return undefined;
      }}
    >
      <Field
        label="标签名"
        hint={fieldErr(err) ?? "最多 12 个字；带「疑似 / 待查 / 观察」的会进「需要处理」"}
      >
        <input
          class="input"
          maxLength={12}
          value={tag}
          aria-invalid={err ? true : undefined}
          onInput={(e) => setTag(e.currentTarget.value)}
        />
      </Field>
      <Field label="备注（选填）" hint="只有后台能看到，最多 200 字">
        <input class="input" maxLength={200} value={note} onInput={(e) => setNote(e.currentTarget.value)} />
      </Field>
    </FormDialog>
  );
}

/* ---------------- 封禁黑名单 ---------------- */
export function BansDialog(props: { onClose: () => void; onChanged: (text: string) => void }) {
  useBusy();
  const [q, setQ] = useState("");
  const dq = useDebounced(q.trim(), 250);
  const { data, error, pending } = usePageData<{ bans: Ban[]; total: number }>(
    `/admin/api/bans${dq ? `?q=${encodeURIComponent(dq)}` : ""}`,
  );
  const [unban, setUnban] = useState<Ban | null>(null);
  const [newId, setNewId] = useState("");
  const [reason, setReason] = useState("");
  const [addErr, setAddErr] = useState("");
  const [adding, setAdding] = useState(false);
  const idId = useId();
  const add = async () => {
    if (!isDiscordId(newId)) {
      setAddErr("Discord ID 必须是 17～20 位纯数字");
      return;
    }
    setAdding(true);
    setAddErr("");
    const r = await addBan(newId.trim(), reason.trim() || "站长手动拉黑");
    setAdding(false);
    if (!r.ok) {
      setAddErr(r.error);
      return;
    }
    props.onChanged(`已将 Discord ID ${newId.trim()} 加入黑名单`);
    setNewId("");
    setReason("");
  };
  const bans = data?.bans ?? [];
  return (
    <>
      <Dialog
        open
        wide
        title="封禁黑名单"
        onClose={props.onClose}
        footer={<Button onClick={props.onClose}>关闭</Button>}
      >
        <div class="xa-form">
          <p class="small muted">
            被拉黑的账号（闲置回收 10-23 起不再拉黑）。移出黑名单后，对方可以重新在 Discord 用 /register 领取
            Key。
          </p>
          <Field label="搜索">
            <input
              class="input"
              type="search"
              placeholder="Discord ID、用户名、昵称或封禁理由"
              value={q}
              onInput={(e) => setQ(e.currentTarget.value)}
            />
          </Field>
          {error && <Notice tone="danger">读取黑名单失败：{error}</Notice>}
          <TableWrap label="封禁黑名单" class={pending ? "xa-busy" : undefined}>
            <table class="table">
              <caption class="sr-only">封禁黑名单</caption>
              <thead>
                <tr>
                  <th scope="col">成员</th>
                  <th scope="col">封禁理由与时间</th>
                  <th scope="col">
                    <span class="sr-only">操作</span>
                  </th>
                </tr>
              </thead>
              <tbody>
                {bans.map((b) => {
                  const title = b.display_name || b.username || b.discord_id;
                  return (
                    <tr key={b.discord_id}>
                      <td>
                        <div class="mb-ident">
                          <Avatar name={title} src={b.avatar} size={28} />
                          <div>
                            <div class="mb-name">{title}</div>
                            <div class="small muted mono">
                              {b.discord_id}
                              {b.username && b.username !== title ? ` · @${b.username}` : ""}
                            </div>
                          </div>
                        </div>
                      </td>
                      <td>
                        <div class="small">{b.reason || "手动封禁"}</div>
                        <div class="small muted">
                          {rel(b.created_at)}
                          {b.banned_by ? ` · 经手人：${b.banned_by}` : ""}
                        </div>
                      </td>
                      <td class="num">
                        <Button size="sm" onClick={() => setUnban(b)}>
                          移出黑名单
                        </Button>
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
            {!bans.length && !pending && <Empty title={dq ? "没有匹配的记录" : "当前没有封禁黑名单记录"} />}
          </TableWrap>
          <h3 class="mb-h4">手动添加封禁</h3>
          <form
            class="mb-banadd"
            noValidate
            onSubmit={(e) => {
              e.preventDefault();
              void add();
            }}
          >
            <Field label="Discord ID（纯数字）" hint={fieldErr(addErr)}>
              <input
                id={idId}
                class="input mono"
                inputMode="numeric"
                autoComplete="off"
                value={newId}
                aria-invalid={addErr ? true : undefined}
                onInput={(e) => setNewId(e.currentTarget.value)}
              />
            </Field>
            <Field label="封禁理由">
              <input
                class="input"
                maxLength={100}
                placeholder="例如：小号刷取、恶意滥用"
                value={reason}
                onInput={(e) => setReason(e.currentTarget.value)}
              />
            </Field>
            <Button variant="danger" type="submit" disabled={adding}>
              加入黑名单
            </Button>
          </form>
        </div>
      </Dialog>
      {unban && (
        <ConfirmDialog
          title="移出黑名单"
          confirmText="移出黑名单"
          onClose={() => setUnban(null)}
          onConfirm={async () => {
            const r = await removeBan(unban.discord_id);
            if (!r.ok) return r.error;
            props.onChanged(
              `已将「${unban.display_name || unban.username || unban.discord_id}」移出黑名单，允许重新领取`,
            );
            return undefined;
          }}
        >
          <p>
            确定将「{unban.display_name || unban.username || unban.discord_id}」移出黑名单？移出后该用户可以在
            Discord 中重新使用 /register 领取 Key。
          </p>
        </ConfirmDialog>
      )}
    </>
  );
}
