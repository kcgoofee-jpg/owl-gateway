import type { MemberPolicy } from "../../lib/member-policy";
/**
 * 首页用到的接口数据形状（Rust 网关返回）。
 * 字段一律可缺省：网关只会多给、不会改名；缺哪个字段就不显示哪块，不编数字。只列页面真正读的字段。
 */

/** /public/live 里的「我的排队」：登录成员才有 */
export type MineItem = { state: "running" } | { state: "waiting"; position: number };
export type LiveMine = { mine?: MineItem[]; waiting?: number; running?: number };

export type Blocked = { code?: string; text?: string };

export type Live = {
  quota?: {
    images_today?: number;
    /** 弧线的比例尺：各账号今日上限合计（手动数字相加）。缺省 = 没有可接图的账号 */
    images_cap?: number;
    v5_today?: number;
  };
  /** full：网关此刻会不会以「排队满」拒绝新图 */
  queue?: { waiting?: number; running?: number; full?: boolean };
  guard?: {
    cooldown?: number;
    blocked?: Blocked | null;
  };
  rules?: {
    queue_cap?: number;
  };
  /** 一个账号平均多久开始一张（秒，出图耗时 + 出完后歇的秒数）：使用指南「生图间隔」建议用 */
  cycle?: number;
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
  /** 「每位成员网页登录一次」闸门生效、该成员还没登录：出图会被 403。 */
  requires_web_login?: boolean;
  key?: string;
  expires_at?: number | null;
  policy?: MemberPolicy;
  today?: {
    images?: number;
    v5?: number;
    legacy?: number;
    daily_images?: number;
    daily_v5?: number;
    /** 全站今天还剩多少张 V5 */
    site_v5_left?: number | null;
    /** 全站每天的 V5 张数 */
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
  /** Durable execution outcome; uncertain rows retain an unconfirmed budget remainder. */
  settlement_status?: string | null;
  remaining_anlas?: number;
  remaining_v5?: number;
  remaining_legacy?: number;
  /** Public correlation token for diagnostics. */
  correlation_id?: string;
  /** 请求来源：本站生图工作台（workbench）或第三方客户端（client） */
  app?: "workbench" | "client";
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
