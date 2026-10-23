/**
 * 额外的限定身份组：和上面的主门槛「满足任意一个即可」（例如类脑「社区建设组」或棱镜「创作者」）。
 * 每条 = 服务器 ID + 身份组 ID + 显示名；单独保存，不和上面的表单混在一起。
 */
import { Plus, X } from "lucide-preact";
import { useEffect, useState } from "preact/hooks";
import { Button, toast } from "../../../../ui";
import { actOrError } from "../_kit";
import { extraChanged, extraRoleProblem, MAX_ROLE_EXTRA, normalizeExtra, type RoleRule } from "./core";

const URL = "/admin/api/ops/registration";
const blank = (): RoleRule => ({ guild: "", role: "", note: "" });

export function ExtraRoles(props: { server: RoleRule[] }) {
  const [rows, setRows] = useState<RoleRule[]>(props.server);
  const [saving, setSaving] = useState(false);
  const serverKey = JSON.stringify(props.server);
  // 服务端的值变了（保存后 / 别处改了）且本地没有未保存的改动：跟着更新
  useEffect(() => {
    if (!extraChanged(rows, JSON.parse(serverKey) as RoleRule[])) return;
    setRows((cur) => (extraChanged(cur, props.server) ? cur : props.server));
  }, [serverKey]);

  const problem = extraRoleProblem(rows);
  const dirty = extraChanged(rows, props.server);
  const set = (i: number, patch: Partial<RoleRule>) =>
    setRows((cur) => cur.map((r, j) => (j === i ? { ...r, ...patch } : r)));
  const save = async () => {
    if (problem) {
      toast(problem, { error: true });
      return;
    }
    setSaving(true);
    const err = await actOrError("PUT", URL, { role_extra: normalizeExtra(rows) }, "额外身份组已保存");
    setSaving(false);
    if (err) toast(err, { error: true });
  };

  return (
    <div class="stack-sm">
      <p class="muted small">
        额外的身份组：和上面的限定身份组<b>满足任意一个就能领 Key</b>
        ，可以来自不同的服务器（例如「类脑」的社区建设组或「棱镜」的创作者）。
      </p>
      {rows.map((r, i) => (
        <div class="form-grid" key={i}>
          <input
            class="input mono"
            inputMode="numeric"
            placeholder="服务器 ID"
            aria-label={`额外身份组 ${i + 1} 的服务器 ID`}
            value={r.guild}
            onInput={(e) => set(i, { guild: e.currentTarget.value })}
          />
          <input
            class="input mono"
            inputMode="numeric"
            placeholder="身份组 ID"
            aria-label={`额外身份组 ${i + 1} 的身份组 ID`}
            value={r.role}
            onInput={(e) => set(i, { role: e.currentTarget.value })}
          />
          <div class="joined">
            <input
              class="input"
              maxLength={60}
              placeholder="显示名，例如 棱镜「创作者」"
              aria-label={`额外身份组 ${i + 1} 的显示名`}
              value={r.note}
              onInput={(e) => set(i, { note: e.currentTarget.value })}
            />
            <Button
              variant="ghost"
              icon
              aria-label={`删除额外身份组 ${i + 1}`}
              onClick={() => setRows((cur) => cur.filter((_, j) => j !== i))}
            >
              <X size={16} aria-hidden="true" />
            </Button>
          </div>
        </div>
      ))}
      {problem && dirty && (
        <span class="field__error" role="alert">
          {problem}
        </span>
      )}
      <div class="row">
        <Button disabled={rows.length >= MAX_ROLE_EXTRA} onClick={() => setRows((cur) => [...cur, blank()])}>
          <Plus size={15} aria-hidden="true" />
          添加一个身份组
        </Button>
        <Button variant="primary" disabled={!dirty || saving} onClick={() => void save()}>
          保存额外身份组
        </Button>
      </div>
    </div>
  );
}
