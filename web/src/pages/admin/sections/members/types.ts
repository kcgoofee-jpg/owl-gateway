/** 成员相关接口的数据形状（/admin/api/members、/admin/api/keys、/admin/api/bans 等）。 */
import type { PolicyBound, PolicySnapshot } from "../member-policy";
export type Tag = { tag: string; note: string; by: string; ts: number };
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
  /** Key 临时暂停的到期时间；null = 没暂停 */
  paused_until: number | null;
  reset_today: { count: number; last_ts: number } | null;
  tags: Tag[];
  /** 服务端算好的待处理 id：paused / expired / expiring / reset_many。空数组表示不用处理。 */
  issues?: string[];
  key_count: number;
  all_key_ids: number[];
};

export type KeyRow = {
  id: number;
  name: string;
  token: string;
  enabled: boolean;
  policy: PolicySnapshot;
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
  total?: number;
  attention?: number;
  filter?: "all" | "attention";
  fields?: Record<string, PolicyBound>;
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
