/**
 * 新建 / 编辑 Key 的表单模型：默认值、从已有 Key 还原、校验并拼出请求体（纯函数，单测覆盖）。
 * 规则与后端一致：数值不能为空 / 为负 / 带小数（整数字段）；0 表示不限；开通 Anlas 时每日 / 每月 Anlas 上限不能都是 0。
 * 新建时的默认值全部来自服务端（/admin/api/ops）：每日 V4.5 / V5 张数和有效天数是后台「领 Key」的设置，
 * Anlas 上限、每分钟请求数是服务端常量；这里不另写一套。每分钟请求数、有效天数默认留空 = 不发给后端（用服务端默认值）。
 */
import type { KeyRow, OpsSnapshot, Role } from "./types";
import { parseNum } from "./validate";

export type KeyForm = {
  name: string;
  role: Role;
  quota: "auto" | "manual";
  scope: "legacy" | "all";
  daily_images: string;
  daily_v5: string;
  daily_anlas: string;
  monthly_anlas: string;
  allow_anlas: boolean;
  exclude_global_v5: boolean;
  featMode: "all" | "custom";
  feats: string[];
  rpm: string;
  expires: string;
  allow_img2img: boolean;
};

export type FormErrors = Partial<Record<keyof KeyForm, string>>;

/**
 * 新建：额度默认取服务端给的（`ops.registration` / `ops.new_key`）；没取到（接口失败）时留空，由人填，不用前端自己的数字。
 * 每分钟请求数和有效天数留空 = 用服务端默认。
 */
export function createDefaults(ops: OpsSnapshot | null): KeyForm {
  const reg = ops?.registration;
  const nk = ops?.new_key;
  const text = (v: number | undefined | null) => (v == null ? "" : String(v));
  // 默认功能为 null = 站长设成了「全部已开放功能」
  const allFeats = ops != null && ops.features.new_key === null;
  return {
    name: "",
    role: "regular",
    quota: "manual",
    scope: "legacy",
    daily_images: text(reg?.daily_images),
    daily_v5: text(reg?.daily_v5),
    daily_anlas: text(nk?.daily_anlas),
    monthly_anlas: text(nk?.monthly_anlas),
    allow_anlas: false,
    exclude_global_v5: false,
    featMode: allFeats ? "all" : "custom",
    feats: ops?.features.new_key ?? [],
    rpm: "",
    expires: "",
    allow_img2img: false,
  };
}

export function formFromKey(k: KeyRow, role: Role): KeyForm {
  return {
    name: k.name,
    role,
    quota: k.quota_auto ? "auto" : "manual",
    scope: k.image_model_scope === "all" ? "all" : "legacy",
    daily_images: String(k.daily_images),
    daily_v5: String(k.daily_v5),
    daily_anlas: String(k.daily_anlas),
    monthly_anlas: String(k.monthly_anlas),
    allow_anlas: k.allow_anlas,
    exclude_global_v5: k.exclude_global_v5,
    featMode: k.features == null ? "all" : "custom",
    feats: k.features ?? [],
    rpm: String(k.rpm),
    expires: "",
    allow_img2img: k.allow_img2img,
  };
}

export type BuildResult = { ok: true; body: Record<string, unknown> } | { ok: false; errors: FormErrors };

export function buildBody(f: KeyForm): BuildResult {
  const errors: FormErrors = {};
  const num = (field: keyof KeyForm, label: string, o: { min?: number; max: number; int?: boolean }) => {
    const r = parseNum(String(f[field]), label, o);
    if (!r.ok) {
      errors[field] = r.error;
      return 0;
    }
    return r.value;
  };
  const manual = f.quota === "manual";
  const body: Record<string, unknown> = {
    name: f.name.trim(),
    role: f.role,
    allow_img2img: f.allow_img2img,
    quota: f.quota,
  };
  // 留空 = 不发：新建用服务端默认（不设到期），编辑保持原值
  if (f.rpm.trim() !== "") body.rpm = num("rpm", "每分钟请求数", { min: 1, max: 600 });
  if (f.expires.trim() !== "") body.expires_days = num("expires", "有效天数", { max: 3650 });
  if (manual) {
    body.image_model_scope = f.scope;
    body.daily_images = num("daily_images", "每日图片张数", { max: 1_000_000 });
    body.daily_v5 = num("daily_v5", "每日 V5 张数", { max: 100_000 });
    const dAnlas = num("daily_anlas", "每日 Anlas 上限", { max: 100_000, int: false });
    const mAnlas = num("monthly_anlas", "每月 Anlas 上限", { max: 100_000, int: false });
    body.daily_anlas = dAnlas;
    body.monthly_anlas = mAnlas;
    body.allow_anlas = f.allow_anlas;
    body.exclude_global_v5 = f.exclude_global_v5;
    if (f.allow_anlas && !errors.daily_anlas && !errors.monthly_anlas && dAnlas <= 0 && mAnlas <= 0) {
      errors.allow_anlas = "开通 Anlas 时，每日和每月 Anlas 上限至少要有一个大于 0";
    }
    if (f.featMode === "custom") {
      if (!f.feats.length) errors.feats = "至少选一个功能，或改选「全部已开放功能」";
      body.features = f.feats;
    } else body.features = null;
  }
  if (f.name.trim().length > 60) errors.name = "名称最长 60 个字";
  return Object.keys(errors).length ? { ok: false, errors } : { ok: true, body };
}

/** 新建时「算法」要在创建后补一次 PATCH；创建接口不认识 quota */
export function createPayload(body: Record<string, unknown>): Record<string, unknown> {
  const { quota: _quota, ...rest } = body;
  return rest;
}
