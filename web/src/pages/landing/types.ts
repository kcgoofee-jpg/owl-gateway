/**
 * 首页用到的接口数据形状（Rust 网关返回）。
 * 字段一律可缺省：网关只会多给、不会改名；缺哪个字段就不显示哪块，不编数字。只列页面真正读的字段。
 */

/** /public/live 里的「我的排队」：登录成员才有 */
export type MineItem = { state: "running" } | { state: "waiting"; position: number; eta: number };
export type LiveMine = { mine?: MineItem[]; waiting?: number; running?: number };

export type Blocked = { code?: string; text?: string };

export type Live = {
  quota?: {
    images_today?: number;
    images_cap?: number;
    v5_today?: number;
    v5_cap?: number;
    /** false = 没有任何上游账号还有免费 V5（网关会拒绝 V5）；缺省按 true */
    upstream_v5_free?: boolean;
    /** true = 站长手动暂停了 V5（网关会拒绝 V5；upstream_v5_free 同时为 false） */
    v5_paused?: boolean;
    v5_access_mode?: "legacy" | "closed" | "restricted" | "everyone";
  };
  /** full：网关此刻会不会以「排队满」拒绝新图 */
  queue?: { waiting?: number; running?: number; full?: boolean };
  guard?: {
    cooldown?: number;
    blocked?: Blocked | null;
    /** 网关的估计：现在再来一张，最早多久后能开始（秒，选号用的同一个数）；没有能接图的账号时为 null */
    start_in?: number | null;
  };
  rules?: {
    queue_cap?: number;
  };
  /** 一个账号平均多久开始一张（秒，出图耗时 + 出完后歇的秒数）：排队时间估算用 */
  cycle?: number;
  /** 上游账号：几个能接图（可用且今天没出满）= 几张图同时出 */
  accounts?: { usable?: boolean; cooling?: number; full?: boolean }[];
  me?: LiveMine | null;
} & Meta;

/** 变化很慢的站点信息：/public/status 与 /public/live 都带（live 里是 30 秒子缓存） */
export type Meta = {
  discord_invite?: string;
  /** 加入按钮跟着它走：注册关了就置灰成「关闭注册」；role_note = 限定身份组的名称（只有设了门槛才有） */
  registration?: { open?: boolean; role_note?: string };
  limits?: {
    key_image_queue?: number;
  };
};

export type Coupon = { id: number; kind?: string; note?: string; expires_at: number; scope?: string };

export type Discord = {
  id?: string;
  username?: string | null;
  display_name?: string | null;
  avatar?: string | null;
};

export type MeGuest = { logged_in: false };
export type MeMember = {
  logged_in: true;
  discord?: Discord | null;
  has_key: boolean;
  /** Current key has already submitted the experience survey. */
  survey_done?: boolean;
  /** 「每位成员网页登录一次」闸门生效、该成员还没登录：出图会被 403。 */
  requires_web_login?: boolean;
  key?: string;
  expires_at?: number | null;
  image_scope?: string;
  today?: {
    images?: number;
    v5?: number;
    legacy?: number;
    daily_images?: number;
    daily_v5?: number;
    /** 全站今天还剩多少张 V5；null = 不受全站限制（全站不设限，或这把 Key 不计入全站） */
    site_v5_left?: number | null;
    /** 全站每天的 V5 张数（与 site_v5_left 同时为 null） */
    site_v5_cap?: number | null;
    /** false = 全站没有上游账号还有免费 V5；缺省按 true */
    upstream_v5_free?: boolean;
    /** true = 站长手动暂停了 V5 */
    v5_paused?: boolean;
    v5_access_allowed?: boolean;
    v5_access_reason?: string;
    anlas?: number;
    daily_anlas?: number;
    reset_hour?: number | null;
  };
  coupons?: Coupon[];
  /** 我的平均出图耗时：只用来补状态卡那句「平均约 N 秒」 */
  timing?: { avg_dur_s?: number | null } | null;
};
export type Me = MeGuest | MeMember;

/** 「我的请求」的一条（/public/me/requests）。 */
export interface MyRequest {
  id: number;
  /** 请求编号（与报错里的 request_id、后台请求日志的「编号」相同）；老记录可能没有 */
  rid: string | null;
  ts: number;
  kind: string;
  model: string;
  status: string;
  /** Server-side response body delivery; absence in legacy rows means unknown. */
  delivery_state?: "transport_complete" | "transport_interrupted" | "pending" | "unknown";
  images: number;
  /** 成员自己花的 Anlas（本站账号代付的记 0） */
  anlas: number;
  site_paid: boolean;
  wait_s: number | null;
  dur_s: number | null;
  reason: string;
  /** 拒绝 / 失败时回给客户端的状态码 */
  code: number | null;
  /** 本站的说明原文（成功时是尺寸与计费说明） */
  message: string;
}

export interface MyRequests {
  items: MyRequest[];
  next: number | null;
  summary: { total: number; ok: number; rejected: number; error: number };
  since: number;
}
