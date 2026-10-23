/**
 * 设置表单的纯函数部分（不依赖外壳，可单测）。
 *
 * 校验规则照后端 `num_int` / `num_float`（crates/gate-server/src/routes/admin）：空值、非数字、小数（整数字段）、越界一律拒绝，
 * 绝不把空值悄悄当成 0——这些字段里 0 往往代表「不限」，一次手滑就会拆掉保护。
 */

export type Raw = string | boolean;

export type NumSpec = {
  kind: "int" | "float";
  label: string;
  min: number;
  max: number;
  hint?: string;
  unit?: string;
};
export type TextSpec = {
  kind: "text";
  label: string;
  /** 字数上限（后端对字符串字段按长度校验） */
  max: number;
  /** 字数下限，缺省 0（可为空） */
  min?: number;
  hint?: string;
  /** 额外格式要求，例如 Discord 数字 ID */
  pattern?: RegExp;
  patternMsg?: string;
};
export type BoolSpec = { kind: "bool"; label: string };
export type SelectSpec = { kind: "select"; label: string; options: Array<{ value: string; label: string }> };
export type FieldSpec = NumSpec | TextSpec | BoolSpec | SelectSpec;

export type Parsed = { ok: true; value: number | string | boolean } | { ok: false; error: string };

/** 把一个原始输入按字段规格解析成值；失败时给出中文原因。 */
export function parseField(spec: FieldSpec, raw: Raw): Parsed {
  if (spec.kind === "bool") return { ok: true, value: raw === true };
  const text = typeof raw === "string" ? raw : String(raw);
  if (spec.kind === "select") {
    return spec.options.some((o) => o.value === text)
      ? { ok: true, value: text }
      : { ok: false, error: `${spec.label}：请从列表里选择` };
  }
  if (spec.kind === "text") {
    const v = text.trim();
    const min = spec.min ?? 0;
    if (v.length < min)
      return { ok: false, error: min === 1 ? `${spec.label}不能为空` : `${spec.label}至少 ${min} 个字` };
    if (v.length > spec.max) return { ok: false, error: `${spec.label}最多 ${spec.max} 个字` };
    if (v && spec.pattern && !spec.pattern.test(v))
      return { ok: false, error: spec.patternMsg ?? `${spec.label}格式不对` };
    return { ok: true, value: v };
  }
  const t = text.trim();
  if (!t) return { ok: false, error: `${spec.label}不能为空` };
  if (!/^[+-]?(\d+\.?\d*|\.\d+)(e[+-]?\d+)?$/i.test(t))
    return { ok: false, error: `${spec.label}必须是数字` };
  const n = Number(t);
  if (!Number.isFinite(n)) return { ok: false, error: `${spec.label}必须是有效数字` };
  if (spec.kind === "int" && !Number.isInteger(n)) return { ok: false, error: `${spec.label}必须是整数` };
  if (n < 0 && spec.min >= 0) return { ok: false, error: `${spec.label}不能是负数` };
  if (n < spec.min || n > spec.max)
    return { ok: false, error: `${spec.label}必须在 ${spec.min}～${spec.max} 之间` };
  return { ok: true, value: n };
}

/** 服务端的值 → 输入框里的原始值 */
export function toRaw(spec: FieldSpec, value: unknown): Raw {
  if (spec.kind === "bool") return value === true || value === 1 || value === "1";
  if (value == null) return "";
  return String(value);
}

/** 两个原始值是否表示同一个值（数字比较数值：「010」与「10」相同） */
export function sameRaw(spec: FieldSpec, a: Raw, b: Raw): boolean {
  if (spec.kind === "int" || spec.kind === "float") {
    const pa = parseField(spec, a);
    const pb = parseField(spec, b);
    if (pa.ok && pb.ok) return pa.value === pb.value;
    // 服务端值本身可能越界（例如读到 0 但下限是 15）：只要文字一样就算没改
    return String(a).trim() === String(b).trim();
  }
  if (spec.kind === "text") return String(a).trim() === String(b).trim();
  return a === b;
}

/** MB / GB 文字：1536 → 「1.5 GB」 */
export function mbText(mb: number): string {
  if (!Number.isFinite(mb)) return "—";
  return mb >= 1024 ? `${(mb / 1024).toFixed(1)} GB` : `${Math.round(mb)} MB`;
}
/** 字节 → GB / MB 文字 */
export function bytesText(bytes: number): string {
  if (!Number.isFinite(bytes)) return "—";
  if (bytes >= 1024 ** 3) return `${(bytes / 1024 ** 3).toFixed(1)} GB`;
  if (bytes >= 1024 ** 2) return `${(bytes / 1024 ** 2).toFixed(1)} MB`;
  return `${Math.round(bytes / 1024)} KB`;
}

/** 0 = 不限 的数字显示 */
/** 新密码规则同后端 change_password：至少 12 个字符，且不能与当前密码或示例密码相同 */
export function passwordProblem(cur: string, next: string, again: string): string | null {
  if (!cur) return "请输入当前密码";
  if (next.length < 12) return "新密码至少需要 12 个字符";
  if (next === "changeme-please") return "新密码不能用示例密码";
  if (next === cur) return "新密码不能与当前密码相同";
  if (next !== again) return "两次输入的新密码不一致";
  return null;
}
