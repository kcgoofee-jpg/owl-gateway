import { useId, useMemo, useState } from "preact/hooks";
import { Button, Dialog, Notice } from "../../../../ui";
import { useBusy } from "../_kit";
import { overridesOf, POLICY_FIELDS, POLICY_SPECS, PolicyFields, policyFromForm } from "../member-policy";
import { toRaw } from "../system/core";
import { useForm } from "../system/forms";
import { call } from "./actions";
import { memberLabel } from "./logic";
import { useCloseGuard } from "./shared";
import type { MemberRow } from "./types";

export function KeyDialog(props: {
  target: MemberRow;
  onClose: () => void;
  onSaved: (message: string) => void;
}) {
  const snapshot = props.target.policy;
  const server = useMemo(() => ({ ...snapshot.effective }), [snapshot]);
  const form = useForm(POLICY_SPECS, server);
  const id = useId();
  const [pending, setPending] = useState(false);
  const [error, setError] = useState("");
  const close = useCloseGuard(form.dirty, props.onClose);
  useBusy();
  const subject = props.target.discord_id ?? props.target.discord?.id;
  const reset = () => {
    for (const name of POLICY_FIELDS) form.set(name, toRaw(POLICY_SPECS[name], snapshot.defaults[name]));
  };
  const save = async () => {
    if (!subject || !form.valid || pending) return;
    setPending(true);
    setError("");
    const effective = policyFromForm(snapshot.effective, form);
    const result = await call(
      "PUT",
      `/admin/api/members/${encodeURIComponent(subject)}/rules`,
      overridesOf(snapshot.defaults, effective),
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
      onClose={close.tryClose}
      footer={
        <>
          <Button onClick={reset}>全部恢复默认</Button>
          <span class="spacer" />
          <Button onClick={close.tryClose}>取消</Button>
          <Button
            type="submit"
            form={id}
            variant="primary"
            disabled={!subject || !form.dirty || !form.valid || pending}
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
        <PolicyFields form={form} defaults={snapshot.defaults} />
        {error && (
          <p class="xa-formerr" role="alert">
            {error}
          </p>
        )}
      </form>
    </Dialog>
  );
}
