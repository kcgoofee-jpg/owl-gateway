/** 临时把指定成员的提示词和成图落到服务器，供排查。默认关闭。 */
import { useMemo, useState } from "preact/hooks";
import { Button, Card, Field } from "../../../../ui";
import { act, useAdmin } from "../../data";

type Item = {
  subject: string;
  session: string;
  rid: string;
  ts: number | null;
  kind: string | null;
  model: string | null;
  detail: string | null;
  prompt: string;
  images: string[];
};

type Payload = { subjects: string[]; items: Item[] };

function parseIds(text: string): string[] {
  return [
    ...new Set(
      text
        .split(/[\s,，]+/)
        .map((s) => s.trim())
        .filter((s) => /^\d{17,20}$/.test(s)),
    ),
  ];
}

export function DebugRecordCard() {
  const { data } = useAdmin<Payload>("/admin/api/ops/debug-record");
  const [draft, setDraft] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const serverText = (data?.subjects ?? []).join("\n");
  const text = draft ?? serverText;
  const ids = useMemo(() => parseIds(text), [text]);
  const dirty = draft !== null && ids.join("\n") !== (data?.subjects ?? []).join("\n");
  if (!data) return null;
  const save = async () => {
    if (saving) return;
    setSaving(true);
    const ok = await act("PUT", "/admin/api/ops/debug-record", { subjects: ids }, "已更新调试记录名单");
    setSaving(false);
    if (ok) setDraft(null);
  };
  return (
    <Card title="临时调试记录" sub="只记录名单内的成员；把提示词、参数和成图写到服务器。用完就清空。">
      <Field label="记录这些成员（每行一个 Discord ID）">
        <textarea
          class="input"
          rows={3}
          value={text}
          placeholder="关闭时留空"
          onInput={(e) => setDraft((e.target as HTMLTextAreaElement).value)}
        />
      </Field>
      <div class="savebar">
        <Button variant="primary" disabled={!dirty || saving} onClick={() => void save()}>
          {saving ? "保存中…" : ids.length ? `保存（${ids.length} 人）` : "关闭记录"}
        </Button>
        {draft !== null && (
          <Button variant="ghost" disabled={saving} onClick={() => setDraft(null)}>
            撤销修改
          </Button>
        )}
      </div>
      <p class="small muted">服务器上已有 {data.items.length} 条记录（提示词 + 成图）。</p>
    </Card>
  );
}
