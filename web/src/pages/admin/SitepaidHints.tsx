/**
 * 后台顶部的「站付 V5 到期」建议：某个上游号快到期、Anlas 还剩不少，可以手动开站付 V5 把它用掉。
 * 只是建议（手动），永远不会自动开；「去设置」弹出设置框，「不再提醒」让这个到期日不再出现。
 */
import { useId, useState } from "preact/hooks";
import { getJSON } from "../../lib/api";
import { fullTime } from "../../lib/format";
import { Badge, Button, Dialog, Field, Notice } from "../../ui";
import { act, useAdmin } from "./Admin";
import { defaultBudget, manualBody, parseBudget, type SitepaidHint, type SitepaidHints } from "./sitepaid";

const WHY = "到期前用掉剩余 Anlas，站长手动开启";

function SetDialog({ hint, onClose }: { hint: SitepaidHint; onClose: () => void }) {
  const id = useId();
  const [budget, setBudget] = useState(defaultBudget(hint));
  const [why, setWhy] = useState(WHY);
  const [err, setErr] = useState("");
  const [pending, setPending] = useState(false);
  const submit = async (e: Event) => {
    e.preventDefault();
    const n = parseBudget(budget);
    if (n == null) return setErr("预算要填大于 0 的数");
    if (!why.trim()) return setErr("请写原因");
    setPending(true);
    setErr("");
    try {
      // 设置项整体替换：已有别的号在站付时不能覆盖它们
      const algo = await getJSON<{ manual?: { paid_v5?: unknown[] } }>("/admin/api/quota-records");
      if ((algo.manual?.paid_v5?.length ?? 0) > 0) {
        return setErr("已有别的号在站付 V5，直接写入会覆盖它；请让 Claude 用 nai-ops 合并设置");
      }
      const r = await act("POST", "/admin/api/ops/manual", manualBody(hint, n, why.trim()), "已开启站付 V5");
      if (r) onClose();
    } catch (x) {
      setErr(x instanceof Error ? x.message : String(x));
    } finally {
      setPending(false);
    }
  };
  return (
    <Dialog
      open
      title="站付 V5（手动）"
      onClose={onClose}
      footer={
        <>
          <Button onClick={onClose}>取消</Button>
          <Button variant="primary" type="submit" form={id} disabled={pending}>
            {pending ? "处理中…" : "开启"}
          </Button>
        </>
      }
    >
      <form id={id} class="xa-form" noValidate onSubmit={submit}>
        <p>{hint.message}</p>
        <Field
          label="最多花多少 Anlas"
          hint={`到 ${fullTime(hint.expires_at)} 为止，免费 V5 用完后用这个号自己的 Anlas`}
        >
          <input
            class="input"
            inputMode="numeric"
            value={budget}
            onInput={(e) => setBudget((e.target as HTMLInputElement).value)}
          />
        </Field>
        <Field label="原因">
          <input class="input" value={why} onInput={(e) => setWhy((e.target as HTMLInputElement).value)} />
        </Field>
        {err && (
          <p class="xa-formerr" role="alert">
            {err}
          </p>
        )}
      </form>
    </Dialog>
  );
}

export function SitepaidBanner() {
  const { data } = useAdmin<SitepaidHints>("/admin/api/sitepaid-hints");
  const [open, setOpen] = useState<SitepaidHint | null>(null);
  const hint = data?.hints?.[0];
  if (!hint) return null;
  return (
    <>
      <Notice tone="warn">
        <Badge title="只是建议，不会自动开启">手动</Badge> {hint.message}
        {hint.stale && "（对账数据偏旧）"}
        <span class="row">
          <Button size="sm" onClick={() => setOpen(hint)}>
            去设置
          </Button>
          <Button
            size="sm"
            variant="ghost"
            onClick={() =>
              act(
                "POST",
                "/admin/api/sitepaid-hints/dismiss",
                { token_id: hint.token_id, expires_at: hint.expires_at },
                "",
              )
            }
          >
            不再提醒
          </Button>
        </span>
      </Notice>
      {open && <SetDialog hint={open} onClose={() => setOpen(null)} />}
    </>
  );
}
