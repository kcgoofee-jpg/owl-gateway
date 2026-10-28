/** 成员相关接口的数据形状（/admin/api/members、/admin/api/keys、/admin/api/bans 等）。 */
import type { PolicySnapshot } from "../member-policy";
export type Tag = { tag: string; note: string; by: string; ts: number };
export type AutoTag = { tag: string; count: number; note?: string };
export type Discord = {
  id: string;
  username: string | null;
  display_name: string | null;
  avatar_url: string;
  profile_url: string;
};

export type Member = {
  id: number;
  name: string;
  enabled: boolean;
  invited: boolean;
  discord_id: string | null;
  discord: Discord | null;
  created_at: number;
  last_used_at: number | null;
  expires_at: number | null;
  policy: PolicySnapshot;
  today: { images: number; legacy_free_images: number; v5: number; anlas: number; requests: number };
  week: { images: number; v5: number; anlas: number; requests: number };
  total_images: number;
  registered_at: number | null;
  first_image_at: number | null;
  last_image_at: number | null;
  rejected_24h: number;
  rejected_why: Array<[string, number]>;
  share: { paused_until: number } | null;
  reset_today: { count: number; last_ts: number } | null;
  tags: Tag[];
  auto_tags: AutoTag[];
  key_count: number;
  all_key_ids: number[];
};

export type KeyRow = {
  id: number;
  name: string;
  token: string;
  enabled: boolean;
  policy: PolicySnapshot;
  invited: boolean;
  expires_at: number | null;
  created_at: number;
  last_used_at: number | null;
  used: { images: number; anlas: number; v5: number; requests: number };
};

/** 成员 + 对应的 Key 行（编辑与展示需要的字段分在两个接口里） */
export type MemberRow = Member & { k?: KeyRow | undefined };

/**
 * 成员列表响应。后端 `?filter=all|attention`（缺省 all = 全量，前端按 chip 在本地筛）：
 * 传 attention 时 `members` 只剩需要处理的行，`total` / `attention` 是裁剪前的计数。
 * 前端目前不传 filter（要全量做 chip 计数与搜索），只把这三个字段当可选契约留着。
 */
export type MembersResp = {
  members: Member[];
  inactivity_days: number;
  total?: number;
  attention?: number;
  filter?: "all" | "attention";
};
export type KeysResp = { keys: KeyRow[] };

export type Ban = {
  discord_id: string;
  created_at: number;
  reason: string;
  username: string;
  display_name: string;
  avatar: string;
  banned_by: string;
};

export type Coupon = {
  id: number;
  kind: string;
  note: string;
  by: string;
  created_at: number;
  expires_at: number;
  used_at: number | null;
  /** 重置哪几项：v45 / v45,v5 / v45,anlas / v45,v5,anlas */
  scope?: string;
};

/* ---------------- 风险线索（/admin/api/abuse） ---------------- */
/** 信号里涉及的一把 Key：Key 已删时只有删除前留下的归属。 */
export type AbuseAccount = {
  key_id: number;
  name: string | null;
  discord_id: string | null;
  discord_name: string | null;
  deleted: boolean;
};
/** 信号涉及的 Discord 账号（没有 Key 的也列出来）。 */
export type AbuseMember = {
  discord_id: string;
  username: string;
  display_name: string;
  avatar_url: string;
  key_id: number | null;
  key_name: string | null;
  banned: boolean;
  ban_reason: string | null;
  first_login: number | null;
  last_login: number | null;
  created_at: number | null;
};
export type AbuseSignal = {
  id: number;
  kind: string;
  kind_label: string;
  subject: string;
  key_ids: string;
  score: number;
  detail: { summary?: string; evidence?: Array<{ k: string; v: unknown }> };
  status: "open" | "confirmed" | "dismissed" | "expired";
  first_ts: number;
  last_ts: number;
  accounts: AbuseAccount[];
  members: AbuseMember[];
};
/** `?status=all` 之外只有对应状态的信号；expired 只出现在 all 里。 */
export type AbuseResp = {
  signals: AbuseSignal[];
  open: number;
  counts: { open: number; confirmed: number; dismissed: number; expired: number; total: number };
  total: number;
  page: number;
  per: number;
};
