import { useId, useMemo, useState } from "preact/hooks";
import { Button, Dialog, Notice } from "../../../../ui";
import { useBusy } from "../_kit";
import {
  overridesOf,
  POLICY_FIELDS,
  type PolicyBound,
  PolicyFields,
  policyFromForm,
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
  const close = useCloseGuard(form.dirty, props.onClose);
  useBusy();
  const subject = props.target.discord_id ?? props.target.discord?.id;
  const reset = () => {
    for (const name of POLICY_FIELDS) {
      const spec = specs[name];
      if (spec) form.set(name, toRaw(spec, snapshot.defaults[name]));
    }
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
          {!snapshot.owner_image && <Button onClick={reset}>全部恢复默认</Button>}
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
        {snapshot.owner_image?.enabled && (
          <p class="small muted">
            全部图片功能已开启；这里只修改个人额度和限速。普通测试请在成员详情点“恢复普通默认”。
          </p>
        )}
        <PolicyFields
          form={form}
          defaults={snapshot.defaults}
          fullImage={snapshot.owner_image?.enabled === true}
        />
        {error && (
          <p class="xa-formerr" role="alert">
            {error}
          </p>
        )}
      </form>
    </Dialog>
  );
}
