import { useId, useMemo, useState } from "preact/hooks";
import { Button, Dialog, Notice } from "../../../../ui";
import { useBusy } from "../_kit";
import {
  type MemberPolicy,
  mergeOverrides,
  POLICY_FIELDS,
  type PolicyBound,
  PolicyFields,
  policySpecs,
} from "../member-policy";
import { toRaw } from "../system/core";
import { useForm } from "../system/forms";
import { call } from "./actions";
import { memberLabel } from "./logic";
import { useCloseGuard } from "./shared";
import type { MemberRow } from "./types";

export function KeyDialog(props: {
  target: MemberRow;
  fields: Record<string, PolicyBound>;
  onClose: () => void;
  onSaved: (message: string) => void;
}) {
  const snapshot = props.target.policy;
  const server = useMemo(() => ({ ...snapshot.effective }), [snapshot]);
  const specs = useMemo(() => policySpecs(props.fields), [props.fields]);
  const form = useForm(specs, server);
  const id = useId();
  const [pending, setPending] = useState(false);
  const [error, setError] = useState("");
  const [resetFields, setResetFields] = useState<Set<keyof MemberPolicy>>(new Set());
  const dirty = form.dirty || [...resetFields].some((name) => snapshot.overrides[name] != null);
  const close = useCloseGuard(dirty, props.onClose);
  useBusy();
  const subject = props.target.discord_id ?? props.target.discord?.id;
  const reset = () => {
    for (const name of POLICY_FIELDS) {
      const spec = specs[name];
      if (spec) form.set(name, toRaw(spec, snapshot.defaults[name]));
    }
    setResetFields(new Set(POLICY_FIELDS));
  };
  const save = async () => {
    if (!subject || !form.valid || pending) return;
    setPending(true);
    setError("");
    const result = await call(
      "PUT",
      `/admin/api/members/${encodeURIComponent(subject)}/rules`,
      mergeOverrides(snapshot.defaults, snapshot.overrides, form.changes(), resetFields),
    );
    setPending(false);
    if (result.ok) props.onSaved("成员规则已保存");
    else setError(result.error);
  };
  return (
    <Dialog
      key={close.gen}
      open
      wide
      title={`${memberLabel(props.target)} · 成员例外`}
      onClose={() => {
        if (!pending) close.tryClose();
      }}
      footer={
        <>
          {!snapshot.owner_image && (
            <Button onClick={reset} disabled={pending}>
              全部恢复默认
            </Button>
          )}
          <span class="spacer" />
          <Button onClick={close.tryClose} disabled={pending}>
            取消
          </Button>
          <Button
            type="submit"
            form={id}
            variant="primary"
            disabled={!subject || !dirty || !form.valid || pending}
          >
            {pending ? "保存中…" : "保存"}
          </Button>
        </>
      }
    >
      {close.asking && (
        <Notice tone="warn">
          <span>放弃未保存的修改？</span>
          <Button size="sm" onClick={close.keep}>
            继续编辑
          </Button>
          <Button size="sm" variant="danger" onClick={close.discard}>
            放弃修改
          </Button>
        </Notice>
      )}
      <form
        id={id}
        {...form.formProps}
        onSubmit={(event) => {
          event.preventDefault();
          void save();
        }}
      >
        {snapshot.owner_image?.enabled && (
          <p class="small muted">站长生图权限已开；这里只改个人额度和限速。</p>
        )}
        <fieldset class="mr-edit-fields" disabled={pending}>
          <PolicyFields
            form={form}
            defaults={snapshot.defaults}
            overrides={snapshot.overrides}
            resetFields={resetFields}
            onReset={(name) => setResetFields((prev) => new Set([...prev, name]))}
            onEdit={(name) =>
              setResetFields((prev) => {
                const next = new Set(prev);
                next.delete(name);
                return next;
              })
            }
            fullImage={snapshot.owner_image?.enabled === true}
          />
        </fieldset>
        {error && (
          <p class="xa-formerr" role="alert">
            {error}
          </p>
        )}
      </form>
    </Dialog>
  );
}
