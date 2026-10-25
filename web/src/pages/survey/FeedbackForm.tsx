/** Lightweight member feedback form; request IDs are verified server-side against this Discord owner. */
import { useState } from "preact/hooks";
import { Button, Card, Notice } from "../../ui";
import { postMember } from "./post";

export function FeedbackForm() {
  const [text, setText] = useState("");
  const [client, setClient] = useState("");
  const [version, setVersion] = useState("");
  const [requestId, setRequestId] = useState("");
  const [busy, setBusy] = useState(false);
  const [done, setDone] = useState(false);
  const [error, setError] = useState("");

  const submit = async (event: Event) => {
    event.preventDefault();
    setBusy(true);
    setDone(false);
    setError("");
    try {
      await postMember("/member/feedback", {
        text,
        client,
        version,
        request_id: requestId.trim(),
      });
      setDone(true);
      setText("");
    } catch (e) {
      setError(e instanceof Error ? e.message : "提交失败，请稍后重试。");
    } finally {
      setBusy(false);
    }
  };

  return (
    <Card title="提交反馈" sub="反馈只有站长能看到。关联请求编号只能用于你自己的请求。">
      <form class="stack" onSubmit={(e) => void submit(e)}>
        <label class="field">
          <span class="field__label">想反馈什么</span>
          <textarea
            class="input"
            name="text"
            required
            minLength={2}
            maxLength={1000}
            rows={4}
            value={text}
            onInput={(e) => setText(e.currentTarget.value)}
          />
        </label>
        <label class="field">
          <span class="field__label">客户端（选填）</span>
          <input
            class="input"
            name="client"
            list="feedback-clients"
            maxLength={80}
            placeholder="例如 柏宝绘、智绘姬，或其他客户端"
            value={client}
            onInput={(e) => setClient(e.currentTarget.value)}
          />
          <datalist id="feedback-clients">
            <option value="柏宝绘" />
            <option value="智绘姬" />
            <option value="NovelAI 官方客户端" />
          </datalist>
        </label>
        <label class="field">
          <span class="field__label">客户端版本（选填）</span>
          <input
            class="input"
            name="version"
            maxLength={40}
            value={version}
            onInput={(e) => setVersion(e.currentTarget.value)}
          />
        </label>
        <label class="field">
          <span class="field__label">关联请求编号（选填）</span>
          <input
            class="input"
            name="request_id"
            inputMode="text"
            pattern="[0-9a-fA-F]{8}"
            maxLength={8}
            placeholder="8 位编号，例如 1a2b3c4d"
            value={requestId}
            onInput={(e) => setRequestId(e.currentTarget.value)}
          />
        </label>
        {done && <Notice tone="ok">反馈已提交，谢谢你。</Notice>}
        {error && <Notice tone="warn">{error}</Notice>}
        <div>
          <Button variant="primary" type="submit" disabled={busy}>
            {busy ? "正在提交…" : "提交反馈"}
          </Button>
        </div>
      </form>
    </Card>
  );
}
