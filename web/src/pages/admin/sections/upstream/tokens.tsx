/** 添加 / 替换上游 Token 的对话框：粘贴后先向 NovelAI 验证（只读，不消耗额度），完整 Token 从不显示。 */
import { useId, useState } from "preact/hooks";
import { ApiError, sendJSON } from "../../../../lib/api";
import { Button, Dialog, toast } from "../../../../ui";
import { tick } from "../../Admin";
import { useBusy } from "../_kit";
import { tokenFormatProblem, type UpstreamAccount } from "./core";

export function TokenDialog(props: {
  mode: "add" | "replace";
  target?: UpstreamAccount | undefined;
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
