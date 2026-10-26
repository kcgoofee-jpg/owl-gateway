/**
 * 新建 / 编辑 Key 的表单模型：默认值、从已有 Key 还原、校验并拼出请求体（纯函数，单测覆盖）。
 * 规则与后端一致（10-27 规范化）：数值不能为空 / 为负 / 带小数（整数字段）；**0 = 停用，上限 9999（每月池 99999），
 * 没有「不限」**；开通 Anlas 时每日 / 每月上限至少一项大于 0。旧数据里的 unlimited 模式在还原时折算成上限值。
 * 新建时的默认值全部来自服务端（/admin/api/ops）：每日 V4.5 / V5 张数和有效天数是后台「领 Key」的设置，
 * Anlas 上限、每分钟请求数是服务端常量；这里不另写一套。每分钟请求数、有效天数默认留空 = 不发给后端（用服务端默认值）。
 */
import type { KeyRow, OpsSnapshot, QuotaLimit, Role } from "./types";
import { parseNum } from "./validate";

export type KeyForm = {
  name: string;
  role: Role;
  quota: "auto" | "manual";
  scope: "legacy" | "all";
  daily_images: string;
  images_mode: "inherit" | "limit";
  daily_v5: string;
  v5_mode: "inherit" | "limit";
  daily_anlas: string;
  daily_anlas_mode: "inherit" | "limit";
  monthly_anlas: string;
  monthly_anlas_mode: "limit";
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
    images_mode: "limit",
    daily_v5: text(reg?.daily_v5),
    v5_mode: "limit",
    daily_anlas: text(nk?.daily_anlas),
    daily_anlas_mode: "inherit",
    monthly_anlas: text(nk?.monthly_anlas),
    monthly_anlas_mode: "limit",
    allow_anlas: false,
    exclude_global_v5: false,
    featMode: allFeats ? "all" : "custom",
    feats: ops?.features.new_key ?? [],
    rpm: "",
    expires: "",
    allow_img2img: false,
  };
}

/** 旧数据里的 unlimited（迁移前的「不限」）折算成上限值；每月池用大上限。 */
const UNLIMITED_FALLBACK = {
  images: 9999,
  v5: 9999,
  daily_anlas: 9999,
  monthly_anlas: 99999,
} as const;

export function formFromKey(k: KeyRow, role: Role): KeyForm {
  const bound = (limit: QuotaLimit, field: keyof typeof UNLIMITED_FALLBACK): QuotaLimit =>
    limit.mode === "unlimited" ? { mode: "limit", value: UNLIMITED_FALLBACK[field] } : limit;
  const fallback = (value: number, auto: boolean, anlas = false): QuotaLimit => {
    if (anlas && value === 0) return { mode: "inherit" };
    if (auto && !anlas) return { mode: "inherit" };
    return { mode: "limit", value };
  };
  const modes = k.quota_limits ?? {
    images: fallback(k.daily_images, k.quota_auto),
    v5: fallback(k.daily_v5, k.quota_auto),
    daily_anlas: fallback(k.daily_anlas, false, true),
    monthly_anlas: fallback(k.monthly_anlas, false),
  };
  const modeName = (limit: QuotaLimit) => (limit.mode === "unlimited" ? "limit" : limit.mode);
  const modeValue = (limit: QuotaLimit, field: keyof typeof UNLIMITED_FALLBACK, fallbackValue: number) =>
    limit.mode === "unlimited"
      ? String(UNLIMITED_FALLBACK[field])
      : limit.mode === "limit"
        ? String(limit.value)
        : String(fallbackValue);
  return {
    name: k.name,
    role,
    quota: k.quota_auto ? "auto" : "manual",
    scope: k.image_model_scope === "all" ? "all" : "legacy",
    daily_images: modeValue(bound(modes.images, "images"), "images", k.daily_images),
    images_mode: modeName(bound(modes.images, "images")) as KeyForm["images_mode"],
    daily_v5: modeValue(bound(modes.v5, "v5"), "v5", k.daily_v5),
    v5_mode: modeName(bound(modes.v5, "v5")) as KeyForm["v5_mode"],
    daily_anlas: modeValue(bound(modes.daily_anlas, "daily_anlas"), "daily_anlas", k.daily_anlas),
    daily_anlas_mode: modeName(bound(modes.daily_anlas, "daily_anlas")) as KeyForm["daily_anlas_mode"],
    monthly_anlas: modeValue(bound(modes.monthly_anlas, "monthly_anlas"), "monthly_anlas", k.monthly_anlas),
    monthly_anlas_mode: modeName(bound(modes.monthly_anlas, "monthly_anlas")) as "limit",
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
    const modeLimit = (
      mode: string,
      field: keyof KeyForm,
      label: string,
      max: number,
      int = true,
    ): QuotaLimit => {
      if (mode !== "limit") return { mode: "inherit" };
      return { mode: "limit", value: num(field, label, { max, int }) };
    };
    const images = modeLimit(f.images_mode, "daily_images", "每日图片张数", 9999);
    const v5 = modeLimit(f.v5_mode, "daily_v5", "每日 V5 张数", 9999);
    const dAnlas = modeLimit(f.daily_anlas_mode, "daily_anlas", "每日 Anlas 上限", 9999, false);
    const mAnlas = modeLimit(f.monthly_anlas_mode, "monthly_anlas", "每月 Anlas 上限", 99999, false);
    body.quota_limits = { images, v5, daily_anlas: dAnlas, monthly_anlas: mAnlas };
    body.allow_anlas = f.allow_anlas;
    body.exclude_global_v5 = f.exclude_global_v5;
    if (f.allow_anlas && !errors.daily_anlas && !errors.monthly_anlas) {
      const cap = (x: QuotaLimit) => (x.mode === "limit" ? x.value : Number.POSITIVE_INFINITY);
      if (cap(dAnlas) === 0 && cap(mAnlas) === 0) {
        errors.allow_anlas = "开通 Anlas 时，每日和每月上限至少一项大于 0（0 = 停用）";
      }
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
