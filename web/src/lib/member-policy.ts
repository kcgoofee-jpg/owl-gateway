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
};
export type MemberOverrides = { [K in keyof MemberPolicy]?: MemberPolicy[K] | null };
export type PolicySnapshot = { defaults: MemberPolicy; overrides: MemberOverrides; effective: MemberPolicy };
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
] as const;
export const POLICY_FIELDS = [...NUMBER_FIELDS, ...FEATURE_FIELDS];

export function overridesOf(defaults: MemberPolicy, effective: MemberPolicy): MemberOverrides {
  return Object.fromEntries(
    POLICY_FIELDS.map((name) => [name, effective[name] === defaults[name] ? null : effective[name]]),
  );
}
export function exceptionCount(overrides: MemberOverrides): number {
  return Object.values(overrides).filter((value) => value != null).length;
}
