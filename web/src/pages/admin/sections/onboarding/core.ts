export type RoleRule = { guild: string; role: string; note: string };
export type ClaimRules = { open: boolean; max_users: number; expires_days: number; roles: RoleRule[] };
export type RegistrationInfo = ClaimRules & { configured: boolean; active: number };
export type Ops = {
  registration: RegistrationInfo;
  features: { global: Record<string, boolean>; labels: Record<string, string> };
  log_retention: { origin_keep_days: number };
};
export const MAX_ROLE_RULES = 6;
export function validRole(rule: RoleRule): boolean {
  const id = (value: string) => /^[1-9][0-9]*$/.test(value) && BigInt(value) <= 18446744073709551615n;
  return id(rule.guild) && id(rule.role) && rule.note.length <= 80;
}
export function claimBody(reg: RegistrationInfo): ClaimRules {
  return { open: reg.open, max_users: reg.max_users, expires_days: reg.expires_days, roles: reg.roles };
}
