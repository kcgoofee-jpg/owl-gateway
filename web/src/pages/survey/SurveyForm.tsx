/**
 * 成员问卷的表单（/survey 页和首页弹窗共用）。身份来自 Discord 登录会话，提交到 /member/survey，不带 Key。
 */
import { useId, useRef, useState } from "preact/hooks";
import { Button, Notice } from "../../ui";
import { displayName } from "../landing/helpers";
import type { MeMember } from "../landing/types";
import { postMember } from "./post";
import "./survey.css";
import { type Answers, type Choice, EMPTY, type Errors, IMPROVE, payload, validate } from "./questions";

/** 单选 / 多选题的一个选项：整行可点，选中高亮，键盘焦点可见。 */
function Option(props: {
  type: "radio" | "checkbox";
  name: string;
  choice: Choice;
  checked: boolean;
  required?: boolean;
  onChange: (checked: boolean) => void;
}) {
  const common = {
    name: props.name,
    value: props.choice.value,
    checked: props.checked,
    onChange: (e: Event) => props.onChange((e.target as HTMLInputElement).checked),
  };
  return (
    // biome-ignore lint/a11y/noLabelWithoutControl: input 在下面的条件分支里（radio / checkbox 两种类型），包在 label 内
    <label class="sv-opt">
      {props.type === "radio" ? (
        <input type="radio" required={props.required === true} {...common} />
      ) : (
        <input type="checkbox" {...common} />
      )}
      <span>{props.choice.label}</span>
    </label>
  );
}

/**
 * 问卷表单：/survey 页和首页弹窗共用。提交成功后显示「已提交」并调用 onDone（弹窗据此稍后关掉）。
 */
export function SurveyForm({ me, onDone }: { me: MeMember; onDone?: () => void }) {
  const [a, setA] = useState<Answers>(EMPTY);
  const [errs, setErrs] = useState<Errors>({});
  const [busy, setBusy] = useState(false);
  const [done, setDone] = useState(false);
  const [fail, setFail] = useState("");
  const ids = { cErr: useId(), cHint: useId() };
  const clientRef = useRef<HTMLInputElement>(null);
  const set = (patch: Partial<Answers>) => {
    setA((cur) => ({ ...cur, ...patch }));
    setDone(false);
  };

  const onSubmit = async (e: Event) => {
    e.preventDefault();
    const found = validate(a);
    setErrs(found);
    setFail("");
    const first = found.client ? clientRef.current : null;
    if (first) {
      first.focus();
      return;
    }
    setBusy(true);
    try {
      await postMember("/member/survey", payload(a));
      setDone(true);
      onDone?.();
    } catch (x) {
      setFail(x instanceof Error ? x.message : "提交失败");
    } finally {
      setBusy(false);
    }
  };

  return (
    <form class="stack" onSubmit={onSubmit} noValidate>
      <div class="card sv-q">
        <label class="field">
          <span class="field__label">
            1. 你主要用哪个客户端接入？<span class="sv-req">（必答）</span>
          </span>
          <input
            class="input"
            type="text"
            name="client"
            ref={clientRef}
            maxLength={60}
            autocomplete="off"
            placeholder="客户端名字"
            aria-required="true"
            aria-invalid={errs.client ? true : undefined}
            aria-describedby={`${ids.cHint}${errs.client ? ` ${ids.cErr}` : ""}`}
            value={a.client}
            onInput={(e) => {
              set({ client: (e.target as HTMLInputElement).value });
              setErrs((x) => ({ ...x, client: "" }));
            }}
          />
          <span class="field__hint" id={ids.cHint}>
            输入具体名字和版本更好，例如「柏宝绘 1.2」「Aaalice NAI 启动器」
          </span>
        </label>
        {errs.client && (
          <p class="lp-err" id={ids.cErr} role="alert">
            {errs.client}
          </p>
        )}
      </div>

      <fieldset class="card sv-q">
        <legend class="sv-legend">2. 你最希望改进哪些地方？（可多选）</legend>
        <p class="sv-hint">不是要更多额度——额度受上游账号物理限制。</p>
        {IMPROVE.map((c) => (
          <Option
            key={c.value}
            type="checkbox"
            name="improve"
            choice={c}
            checked={a.improve.includes(c.value)}
            onChange={(on) =>
              set({ improve: on ? [...a.improve, c.value] : a.improve.filter((v) => v !== c.value) })
            }
          />
        ))}
        <label class="field sv-other">
          <span class="field__label">其他想法（选填）</span>
          <textarea
            class="textarea"
            name="improve_other"
            maxLength={300}
            placeholder="具体说一处让你不爽的体验，最好带当时的截图或报错编号"
            value={a.improveOther}
            onInput={(e) => set({ improveOther: (e.target as HTMLTextAreaElement).value })}
          />
        </label>
      </fieldset>

      <div class="card sv-q stack">
        <p class="muted small">
          身份来自你的 Discord 登录（{displayName(me)}），一人一份；再次提交会覆盖之前的回答。
        </p>
        {fail && <Notice tone="danger">{fail}</Notice>}
        {done && (
          <p class="sv-ok" role="status">
            已提交，谢谢你的反馈！
          </p>
        )}
        <div>
          <Button type="submit" variant="primary" size="lg" disabled={busy}>
            {busy ? "提交中…" : done ? "更新我的回答" : "提交问卷"}
          </Button>
        </div>
      </div>
    </form>
  );
}
