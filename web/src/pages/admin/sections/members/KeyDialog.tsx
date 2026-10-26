/** 新建 / 编辑 Key 的对话框。校验规则见 keyForm.ts；出错信息显示在框里（原生 dialog 会盖住 toast）。 */
import type { ComponentChildren } from "preact";
import { useEffect, useId, useMemo, useRef, useState } from "preact/hooks";
import { getJSON } from "../../../../lib/api";
import { Button, Dialog, Field, Notice, Skeleton, toast } from "../../../../ui";
import { useBusy } from "../_kit";
import { FEATURES } from "../features";
import { call } from "./actions";
import {
  buildBody,
  createDefaults,
  createPayload,
  type FormErrors,
  formFromKey,
  type KeyForm,
} from "./keyForm";
import { roleOf } from "./logic";
import { useCloseGuard } from "./shared";
import { type KeyRow, type MemberRow, type OpsSnapshot, ROLE_LABEL, type Role } from "./types";

/** 接口没给功能名表时的兜底（后端 features.FEATURES 的副本）。 */
const DEFAULT_FEATURE_LABELS: Record<string, string> = Object.fromEntries(
  FEATURES.map((f) => [f.id, f.label]),
);

type Props = {
  /** null = 新建 */
  target: MemberRow | null;
  /** 打开即选「手动」（从成员详情里点「手动」进来） */
  startManual?: boolean;
  idleDays: number;
  onClose: () => void;
  onSaved: (text: string) => void;
  onCreated: (key: KeyRow) => void;
};

function roleHint(role: Role, idleDays: number): string {
  const idle = idleDays > 0 ? `满 ${idleDays} 天未生图会被闲置回收` : "闲置回收当前已关闭";
  switch (role) {
    case "regular":
      return `参与成员统计，跟随全站额度；${idle}`;
    case "invite":
      return "站长邀请特批，豁免闲置回收，只由站长手动删除";
  }
}

function NumField(props: {
  label: string;
  hint?: ComponentChildren;
  value: string;
  error: string | undefined;
  onInput: (v: string) => void;
  min?: number;
  max?: number;
  step?: number | "any";
  placeholder?: string | undefined;
  disabled?: boolean;
}) {
  return (
    <Field
      label={props.label}
      hint={
        props.error ? (
          <span class="field__err" role="alert">
            {props.error}
          </span>
        ) : (
          props.hint
        )
      }
    >
      <input
        class="input"
        type="number"
        inputMode="decimal"
        min={props.min ?? 0}
        max={props.max}
        step={props.step ?? 1}
        disabled={props.disabled}
        value={props.value}
        placeholder={props.placeholder}
        aria-invalid={props.error ? true : undefined}
        onInput={(e) => props.onInput(e.currentTarget.value)}
      />
    </Field>
  );
}

export function KeyDialog(props: Props) {
  useBusy();
  const { target } = props;
  const editing = target !== null;
  const row = target?.k;
  const formId = useId();
  const [ops, setOps] = useState<OpsSnapshot | null>(null);
  const [opsDone, setOpsDone] = useState(editing); // 编辑时不必等 /ops；功能名用内置副本
  const [form, setForm] = useState<KeyForm | null>(null);
  const initial = useRef<string>("");
  const [errors, setErrors] = useState<FormErrors>({});
  const [formErr, setFormErr] = useState("");
  const [pending, setPending] = useState(false);

  useEffect(() => {
    getJSON<OpsSnapshot>("/admin/api/ops")
      .then(setOps)
      .catch(() => setOps(null))
      .finally(() => setOpsDone(true));
  }, []);

  useEffect(() => {
    if (form || !opsDone) return;
    let f: KeyForm;
    if (editing) {
      if (!row || !target) return;
      f = formFromKey(row, roleOf(target));
    } else f = createDefaults(ops);
    if (props.startManual) f.quota = "manual";
    initial.current = JSON.stringify(f);
    setForm(f);
  }, [opsDone, ops, form, editing, row, target, props.startManual]);

  const dirty = form !== null && JSON.stringify(form) !== initial.current;
  const guard = useCloseGuard(dirty, props.onClose);
  const labels = ops?.features.labels ?? DEFAULT_FEATURE_LABELS;
  const featureIds = useMemo(() => Object.keys(labels), [labels]);

  const set = <K extends keyof KeyForm>(k: K, v: KeyForm[K]) => {
    setForm((f) => (f ? { ...f, [k]: v } : f));
    setErrors((e) => (e[k] ? { ...e, [k]: undefined } : e));
  };
  const setRole = (role: Role) => setForm((f) => (f ? { ...f, role } : f));

  const submit = async () => {
    if (!form || pending) return;
    setFormErr("");
    const r = buildBody(form);
    if (!r.ok) {
      setErrors(r.errors);
      setFormErr("有填写不正确的地方，请按提示修改");
      return;
    }
    setErrors({});
    setPending(true);
    try {
      if (editing && target) {
        const res = await call<{ key: KeyRow }>("PATCH", `/admin/api/keys/${target.id}`, r.body);
        if (!res.ok) return setFormErr(res.error);
        props.onSaved(`已保存「${form.name.trim() || target.name}」`);
      } else {
        const res = await call<{ key: KeyRow }>("POST", "/admin/api/keys", createPayload(r.body));
        if (!res.ok) return setFormErr(res.error);
        if (form.quota === "auto") {
          const p = await call("PATCH", `/admin/api/keys/${res.data.key.id}`, { quota: "auto" });
          if (!p.ok) toast(`Key 已创建，但改为统一额度失败：${p.error}`, { error: true });
        }
        props.onCreated(res.data.key);
      }
    } finally {
      setPending(false);
    }
  };

  const f = form;
  const manual = f?.quota === "manual";
  // 新建时有效天数留空 = 「新成员默认」有效期（后台「领 Key」的设置）
  const defaultExpiry = ops?.registration.expires_days
    ? `默认 ${ops.registration.expires_days} 天`
    : "不设到期";
  return (
    <Dialog
      key={guard.gen}
      open
      title={editing ? `编辑：${target?.name ?? ""}` : "新建 Key"}
      onClose={guard.tryClose}
      footer={
        <>
          <Button onClick={guard.tryClose}>取消</Button>
          <Button variant="primary" type="submit" form={formId} disabled={!f || pending}>
            {pending ? "保存中…" : editing ? "保存" : "创建"}
          </Button>
        </>
      }
    >
      {guard.asking && (
        <div class="mb-ask">
          <Notice tone="warn">
            <span>有未保存的修改，确定放弃吗？</span>
            <span class="spacer" />
            <Button size="sm" onClick={guard.keep}>
              继续编辑
            </Button>
            <Button size="sm" variant="danger" onClick={guard.discard}>
              放弃修改
            </Button>
          </Notice>
        </div>
      )}
      {!f ? (
        <div class="stack" aria-busy="true">
          <Skeleton h="40px" />
          <Skeleton h="120px" />
        </div>
      ) : editing && !row ? (
        <Notice tone="danger">这把 Key 的详细信息读取失败，请稍后再试。</Notice>
      ) : (
        <form
          id={formId}
          class="xa-form"
          noValidate
          onSubmit={(e) => {
            e.preventDefault();
            void submit();
          }}
        >
          {!editing && !ops && (
            <Notice tone="warn">读取默认额度失败：下面的额度请手动填写，或关掉对话框稍后再试。</Notice>
          )}
          <Field
            label="名称 / 备注"
            hint={
              errors.name ? (
                <span class="field__err" role="alert">
                  {errors.name}
                </span>
              ) : undefined
            }
          >
            <input
              class="input"
              maxLength={60}
              placeholder="例如：张三"
              value={f.name}
              onInput={(e) => set("name", e.currentTarget.value)}
            />
          </Field>

          <fieldset class="mb-fieldset">
            <legend class="xa-form__sec">身份组</legend>
            <div class="xa-form__opts">
              {(["regular", "invite"] as Role[]).map((r) => (
                <label class="xa-check" key={r}>
                  <input type="radio" name="role" checked={f.role === r} onChange={() => setRole(r)} />
                  <span>
                    {ROLE_LABEL[r]}
                    <small>{roleHint(r, props.idleDays)}</small>
                  </span>
                </label>
              ))}
            </div>
          </fieldset>

          <fieldset class="mb-fieldset">
            <legend class="xa-form__sec">额度来源</legend>
            <div class="xa-form__opts">
              <label class="xa-check">
                <input
                  type="radio"
                  name="quota"
                  checked={f.quota === "auto"}
                  onChange={() => set("quota", "auto")}
                />
                <span>
                  统一
                  <small>跟随「额度与保护」里的每人每日额度，和其他成员一样</small>
                </span>
              </label>
              <label class="xa-check">
                <input
                  type="radio"
                  name="quota"
                  checked={f.quota === "manual"}
                  onChange={() => set("quota", "manual")}
                />
                <span>
                  单独设<small>下面填多少就是多少，不跟随全站额度</small>
                </span>
              </label>
            </div>
          </fieldset>

          {manual && (
            <>
              <Field label="图片模型范围">
                <select
                  class="select"
                  value={f.scope}
                  onChange={(e) => set("scope", e.currentTarget.value === "all" ? "all" : "legacy")}
                >
                  <option value="legacy">仅 V4.5 及更低（默认）</option>
                  <option value="all">所有当前支持模型（含 V5）</option>
                </select>
              </Field>
              <div class="xa-form__grid">
                <Field label="每日 V4.5 额度模式">
                  <select
                    class="select"
                    value={f.images_mode}
                    onChange={(e) => set("images_mode", e.currentTarget.value as KeyForm["images_mode"])}
                  >
                    <option value="inherit">继承统一默认</option>
                    <option value="limit">限制数量</option>
                  </select>
                </Field>
                <NumField
                  label="每日 V4.5 及以下免费图张数"
                  hint={f.images_mode === "limit" ? "0 = 停用；上限 9999" : "仅在限制数量模式生效"}
                  value={f.daily_images}
                  error={errors.daily_images}
                  onInput={(v) => set("daily_images", v)}
                  max={9999}
                  disabled={f.images_mode !== "limit"}
                />
                <Field label="每日 V5 额度模式">
                  <select
                    class="select"
                    value={f.v5_mode}
                    onChange={(e) => set("v5_mode", e.currentTarget.value as KeyForm["v5_mode"])}
                  >
                    <option value="inherit">继承统一默认</option>
                    <option value="limit">限制数量</option>
                  </select>
                </Field>
                <NumField
                  label="每日 V5 张数"
                  hint={
                    f.v5_mode === "limit"
                      ? "0 = 停用 V5；上限 9999；仍受全站 V5 总量约束"
                      : "仅在限制数量模式生效"
                  }
                  value={f.daily_v5}
                  error={errors.daily_v5}
                  onInput={(v) => set("daily_v5", v)}
                  max={9999}
                  disabled={f.v5_mode !== "limit"}
                />
                <Field label="每日 Anlas 额度模式">
                  <select
                    class="select"
                    value={f.daily_anlas_mode}
                    onChange={(e) =>
                      set("daily_anlas_mode", e.currentTarget.value as KeyForm["daily_anlas_mode"])
                    }
                  >
                    <option value="inherit">继承每人默认</option>
                    <option value="limit">限制额度</option>
                  </select>
                </Field>
                <NumField
                  label="每日 Anlas 上限"
                  hint={
                    f.daily_anlas_mode === "limit"
                      ? "0 = 停用（不允许消耗 Anlas）；上限 9999"
                      : "仅在限制额度模式生效"
                  }
                  value={f.daily_anlas}
                  error={errors.daily_anlas}
                  step="any"
                  max={9999}
                  onInput={(v) => set("daily_anlas", v)}
                  disabled={f.daily_anlas_mode !== "limit"}
                />
                <Field label="每月 Anlas 额度模式">
                  <select
                    class="select"
                    value={f.monthly_anlas_mode}
                    onChange={(e) =>
                      set("monthly_anlas_mode", e.currentTarget.value as KeyForm["monthly_anlas_mode"])
                    }
                  >
                    <option value="limit">限制额度</option>
                  </select>
                </Field>
                <NumField
                  label="每月 Anlas 上限"
                  hint={
                    f.monthly_anlas_mode === "limit" ? "0 = 本月停用；上限 99999" : "仅在限制额度模式生效"
                  }
                  value={f.monthly_anlas}
                  error={errors.monthly_anlas}
                  step="any"
                  max={99999}
                  onInput={(v) => set("monthly_anlas", v)}
                  disabled={f.monthly_anlas_mode !== "limit"}
                />
              </div>
              <div class="xa-form__opts">
                <label class="xa-check">
                  <input
                    type="checkbox"
                    checked={f.allow_anlas}
                    aria-invalid={errors.allow_anlas ? true : undefined}
                    onChange={(e) => set("allow_anlas", e.currentTarget.checked)}
                  />
                  <span>
                    允许消耗 Anlas
                    <small>
                      旧模型和 V5 的付费规格都受每日 / 每月 Anlas
                      上限和全站预算约束；开通时两个上限至少要有一个大于 0
                    </small>
                  </span>
                </label>
                {errors.allow_anlas && (
                  <p class="field__err" role="alert">
                    {errors.allow_anlas}
                  </p>
                )}
                <label class="xa-check">
                  <input
                    type="checkbox"
                    checked={f.exclude_global_v5}
                    onChange={(e) => set("exclude_global_v5", e.currentTarget.checked)}
                  />
                  <span>
                    不占全站 V5 额度<small>这把 Key 的 V5 不计入、也不受全站 V5 日额度限制</small>
                  </span>
                </label>
              </div>

              <fieldset class="mb-fieldset">
                <legend class="xa-form__sec">功能权限</legend>
                <div class="xa-form__opts">
                  <label class="xa-check">
                    <input
                      type="radio"
                      name="featMode"
                      checked={f.featMode === "all"}
                      onChange={() => set("featMode", "all")}
                    />
                    <span>全部已开放功能（含以后新开放的）</span>
                  </label>
                  <label class="xa-check">
                    <input
                      type="radio"
                      name="featMode"
                      checked={f.featMode === "custom"}
                      onChange={() => set("featMode", "custom")}
                    />
                    <span>仅选中的功能</span>
                  </label>
                  {f.featMode === "custom" && (
                    <fieldset class="mb-feats">
                      <legend class="sr-only">可用功能</legend>
                      {featureIds.map((id) => (
                        <label class="xa-check" key={id}>
                          <input
                            type="checkbox"
                            checked={f.feats.includes(id)}
                            onChange={(e) =>
                              set(
                                "feats",
                                e.currentTarget.checked ? [...f.feats, id] : f.feats.filter((x) => x !== id),
                              )
                            }
                          />
                          <span>{labels[id] ?? id}</span>
                        </label>
                      ))}
                    </fieldset>
                  )}
                  {errors.feats && (
                    <p class="field__err" role="alert">
                      {errors.feats}
                    </p>
                  )}
                </div>
              </fieldset>
            </>
          )}

          <div class="xa-form__sec">其他</div>
          <div class="xa-form__grid">
            <NumField
              label="每分钟请求数（RPM）"
              hint={editing ? "1～600" : `1～600；留空 = 默认 ${ops?.new_key?.rpm ?? "（服务端默认）"}`}
              min={1}
              value={f.rpm}
              error={errors.rpm}
              onInput={(v) => set("rpm", v)}
            />
            <NumField
              label="有效天数"
              hint={editing ? "留空保持原有效期；0 = 永久" : `留空 = ${defaultExpiry}；0 = 永久`}
              value={f.expires}
              error={errors.expires}
              placeholder={editing ? "留空保持原有效期" : defaultExpiry}
              onInput={(v) => set("expires", v)}
            />
          </div>
          <div class="xa-form__opts">
            <label class="xa-check">
              <input
                type="checkbox"
                checked={f.allow_img2img}
                onChange={(e) => set("allow_img2img", e.currentTarget.checked)}
              />
              <span>
                允许 img2img / 局部重绘
                <small>会扣 Anlas 的请求还要允许消耗 Anlas</small>
              </span>
            </label>
          </div>
          {formErr && (
            <p class="xa-formerr" role="alert">
              {formErr}
            </p>
          )}
        </form>
      )}
    </Dialog>
  );
}
