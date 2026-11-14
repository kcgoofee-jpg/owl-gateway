export type MemberPolicy = {
  daily_images: number;
  daily_v5: number;
  daily_anlas: number;
  monthly_anlas: number;
  rpm: number;
  image: boolean;
  v5: boolean;
  vibe: boolean;
  tags: boolean;
  upscale: boolean;
  augment: boolean;
  img2img: boolean;
  paid_parameters: boolean;
  share_exempt: boolean;
};
export type MemberOverrides = { [K in keyof MemberPolicy]?: MemberPolicy[K] | null };
export type PatronView = {
  enabled: boolean;
  queue_jump: boolean;
  self_service: boolean;
  has_password: boolean;
  portal_mode?: "basic" | "full";
  bound_token_id?: string;
};
export type PolicySnapshot = {
  defaults: MemberPolicy;
  overrides: MemberOverrides;
  effective: MemberPolicy;
  owner_image?: { enabled: boolean };
  patron?: PatronView;
};
export const NUMBER_FIELDS = ["daily_images", "daily_v5", "daily_anlas", "monthly_anlas", "rpm"] as const;
export const FEATURE_FIELDS = [
  "image",
  "v5",
  "vibe",
  "tags",
  "img2img",
  "upscale",
  "augment",
  "paid_parameters",
  "share_exempt",
] as const;
export const FEATURE_COPY: Record<(typeof FEATURE_FIELDS)[number], { label: string; help: string }> = {
  image: { label: "图片生成", help: "关闭后这把 Key 不能出图。" },
  v5: { label: "V5", help: "关闭后不能选 V5 模型。" },
  vibe: { label: "Vibe 与参考图", help: "参考图编码和 Vibe 传输。" },
  tags: { label: "标签补全", help: "提示词补全接口。" },
  img2img: { label: "图生图与局部重绘", help: "用已有图片再画，或涂抹重绘。" },
  upscale: { label: "放大", help: "官方放大工具。" },
  augment: { label: "导演工具", help: "Line Art、Colorize 一类后处理。" },
  paid_parameters: {
    label: "高规格生图（大图、多步数与批量）",
    help: "不再按免费档裁剪。大图 / 多步 / 批量走全站 Anlas / V5 池。图生图、导演工具和放大仍要单独开。",
  },
  share_exempt: {
    label: "分享豁免",
    help: "跳过「先分享再用图」检查；额度、封禁与其他限制不受影响。",
  },
};
export const POLICY_FIELDS = [...NUMBER_FIELDS, ...FEATURE_FIELDS];

export function overridesOf(defaults: MemberPolicy, effective: MemberPolicy): MemberOverrides {
  return Object.fromEntries(
    POLICY_FIELDS.map((name) => [name, effective[name] === defaults[name] ? null : effective[name]]),
  );
}
/** 只改动用户编辑或明确恢复的字段，保留其余已设置的例外。 */
export function mergeOverrides(
  defaults: MemberPolicy,
  existing: MemberOverrides,
  changes: Record<string, number | string | boolean>,
  reset: ReadonlySet<keyof MemberPolicy>,
): MemberOverrides {
  const result: MemberOverrides = { ...existing };
  for (const name of POLICY_FIELDS) {
    if (name in changes) {
      const value = changes[name];
      Object.assign(result, { [name]: value === defaults[name] ? null : value });
    } else if (reset.has(name)) {
      Object.assign(result, { [name]: null });
    }
  }
  return result;
}
export function exceptionCount(overrides: MemberOverrides): number {
  return Object.values(overrides).filter((value) => value != null).length;
}
