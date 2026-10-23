/** 后台密码：改完所有已登录会话（包括当前）立即失效，重新登录。 */
import { Eye, EyeOff } from "lucide-preact";
import { useId, useState } from "preact/hooks";
import { ApiError, sendJSON } from "../../../../lib/api";
import { Button, Card, toast } from "../../../../ui";
import { useBusy } from "../_kit";
import { passwordProblem } from "./core";

function PwInput(props: {
  label: string;
  value: string;
  onInput: (v: string) => void;
  autoComplete: string;
  hint?: string;
}) {
  const id = useId();
  const [show, setShow] = useState(false);
  return (
    <div class="field">
      <label class="field__label" for={id}>
        {props.label}
      </label>
      <div class="pwbox">
        <input
          id={id}
          class="input"
          type={(show ? "text" : "password") as "text"}
          autoComplete={props.autoComplete}
          value={props.value}
          aria-describedby={props.hint ? `${id}-h` : undefined}
          onInput={(e) => props.onInput((e.target as HTMLInputElement).value)}
        />
        <Button
          variant="ghost"
          icon
          aria-label={show ? `隐藏${props.label}` : `显示${props.label}`}
          aria-pressed={show}
          onClick={() => setShow((s) => !s)}
        >
          {show ? <EyeOff size={16} aria-hidden="true" /> : <Eye size={16} aria-hidden="true" />}
        </Button>
      </div>
      {props.hint && (
        <span class="field__hint" id={`${id}-h`}>
          {props.hint}
        </span>
      )}
    </div>
  );
}

export default function PasswordCard() {
  const [cur, setCur] = useState("");
  const [next, setNext] = useState("");
  const [again, setAgain] = useState("");
  const [err, setErr] = useState("");
  const [pending, setPending] = useState(false);
  const [done, setDone] = useState(false);
  const dirty = !!(cur || next || again);
  useBusy(dirty);

  const submit = async () => {
    const problem = passwordProblem(cur, next, again);
    if (problem) {
      setErr(problem);
      return;
    }
    setErr("");
    setPending(true);
    try {
      await sendJSON("PUT", "/admin/api/password", { current: cur, new: next });
      setDone(true);
      setCur("");
      setNext("");
      setAgain("");
      toast("密码已修改，请用新密码重新登录");
      // 所有会话（包括当前）都已失效：重新载入，外壳会回到登录页
      setTimeout(() => location.reload(), 1500);
    } catch (e) {
      // 注意：这里的 401 是「当前密码不正确」，不是会话过期，所以不走 act()（它会把 401 当成掉线踢回登录页）
      setErr(e instanceof ApiError || e instanceof Error ? e.message : String(e));
    } finally {
      setPending(false);
    }
  };

  return (
    <Card title="后台密码" sub="修改后所有已登录的会话（包括当前）都会失效，需要用新密码重新登录。">
      <form
        class="stack-sm pwform"
        autoComplete="off"
        onSubmit={(e) => {
          e.preventDefault();
          void submit();
        }}
      >
        {/* 隐藏的用户名字段：让密码管理器把新密码和本站关联保存 */}
        <input type="text" name="username" autoComplete="username" value="admin" hidden readOnly />
        <PwInput label="当前密码" value={cur} onInput={setCur} autoComplete="current-password" />
        <PwInput
          label="新密码"
          value={next}
          onInput={setNext}
          autoComplete="new-password"
          hint="至少 12 个字符，且不能与当前密码相同"
        />
        <PwInput label="确认新密码" value={again} onInput={setAgain} autoComplete="new-password" />
        {err && (
          <p class="field__error" role="alert">
            {err}
          </p>
        )}
        {done && (
          <p class="small" role="status">
            密码已修改，正在返回登录页…
          </p>
        )}
        <div class="savebar">
          <Button type="submit" variant="primary" disabled={pending || done || !dirty}>
            {pending ? "修改中…" : "修改密码"}
          </Button>
        </div>
        <p class="muted small">
          忘记密码时，可在服务器的 .env 里设置 ADMIN_PASSWORD，并删除数据库里的 admin_password_hash
          设置来重置。
        </p>
      </form>
    </Card>
  );
}
