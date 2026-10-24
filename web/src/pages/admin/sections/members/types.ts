/** 成员相关接口的数据形状（/admin/api/members、/admin/api/keys、/admin/api/bans 等）。 */
import type { Ops } from "../onboarding/core";

export type Role = "regular" | "invite";
export const ROLE_LABEL: Record<Role, string> = {
  regular: "普通",
  invite: "邀请",
};

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
  role: Role;
  role_label: string;
  discord_id: string | null;
  discord: Discord | null;
  created_at: number;
  last_used_at: number | null;
  expires_at: number | null;
  daily_images: number;
  daily_v5: number;
  allow_anlas: boolean;
  daily_anlas: number;
  image_model_scope: string;
  quota_auto: boolean;
  today: { images: number; v5: number; anlas: number; requests: number };
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
  daily_images: number;
  daily_anlas: number;
  daily_v5: number;
  monthly_anlas: number;
  rpm: number;
  allow_anlas: boolean;
  allow_img2img: boolean;
  exclude_global_v5: boolean;
  image_model_scope: string;
  quota_auto: boolean;
  role: Role;
  /** null = 全部已开放功能；数组 = 仅这些功能 */
  features: string[] | null;
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

export type OpsSnapshot = Pick<Ops, "registration" | "features" | "new_key">;
