/** 日志相关接口的数据形状。 */

export type Paged = { page: number; per_page: number; total: number; pages: number };

export type AuditItem = {
  id: number;
  ts: number;
  /** 请求编号（与请求日志的「编号」相同）；10-23 之前的记录为空串 */
  rid: string;
  key_id: number | null;
  key_name: string;
  kind: string;
  model: string;
  status: string;
  prompt: string;
  negative: string;
  /** JSON 字符串：{params, char_prompts, char_negatives} */
  extra: string;
  has_image: boolean | number;
  has_thumb: boolean | number;
};
export type AuditResp = Paged & { items: AuditItem[] };

export type AuditExtra = {
  params?: {
    width?: number;
    height?: number;
    seed?: number;
    sampler?: string;
    steps?: number;
    scale?: number;
  };
  char_prompts?: string[];
  char_negatives?: string[];
};

export type UsageLog = {
  id: number;
  ts: number;
  key_id: number | null;
  key_name: string;
  kind: string;
  model: string;
  status: "ok" | "rejected" | "error" | string;
  images: number;
  anlas: number;
  tokens: number;
  unconfirmed_anlas: number;
  detail: string;
  reason: string;
  /** 拒绝原因的短说明（服务端算好） */
  reason_label: string;
  wait_ms: number;
  dur_ms: number;
  up_status: number;
  rid: string;
  correlation_id?: string;
  /** 来源 IP（原文）；保留期外是空串 */
  src: string;
  /** User-Agent 原文；保留期外是空串 */
  client: string;
  /** 处理这条请求的上游账号；没选号就被拒为 null */
  token_id: string | null;
  /** 客户端特征（防一人多号）：X-Requested-With、Accept-Language、Sec-CH-UA、请求头指纹、浏览器标记哈希、Origin；没有为 null */
  xrw?: string | null;
  lang?: string | null;
  chua?: string | null;
  hfp?: string | null;
  bid?: string | null;
  origin?: string | null;
};
/** 同一组筛选条件下各状态的条数（不受 status 筛选影响） */
export type LogSummary = { total: number; ok: number; rejected: number; error: number };
export type LogsResp = Paged & {
  logs: UsageLog[];
  summary: LogSummary;
  /** 筛选下拉框的原因码和短说明（服务端给） */
  reasons?: Array<{ code: string; label: string }>;
};

export type ActionRow = {
  id: number;
  ts: number;
  actor: string;
  action: string;
  target: string;
  detail: string;
  ok: number | boolean;
  /** 后端按操作者分的类：manual = 站长 / Claude，auto = 算法 / 定时任务，member = 成员自己，"" = 认不出 */
  kind?: "manual" | "auto" | "member" | "";
};
export type ActionsResp = Paged & { actions: ActionRow[]; action_kinds: string[] };

export type BugRow = {
  sig: string;
  source: string;
  level: "error" | "warn";
  title: string;
  detail: string;
  count: number;
  first_seen: number;
  last_seen: number;
  last_rid: string;
  last_key: number | null;
  last_path: string;
  resolved_at: number | null;
  /** 自动处理的原因码（见 format.ts 的 autoReason）；站长手动处理 / 未处理为空 */
  auto_resolved?: string | null;
  key_name: string;
};
export type BugsResp = { errors: BugRow[]; open: number };

export type FeedbackItem = {
  id: number;
  ts: number;
  discord_id: string;
  username: string;
  key_id: number | null;
  key_name: string;
  /** 显示用的文字：新记录是成员写的话；旧版选择题的记录是一行摘要「（旧版选择题）整体：…；困扰：…」；什么都没有是空串 */
  text: string;
  /** 是不是旧版选择题的记录（text 是服务端拼的摘要） */
  legacy: boolean;
  client?: string;
  version?: string;
  request_id?: string;
  reviewed_at?: number | null;
  reviewed_by?: string | null;
};
export type FeedbackResp = { items: FeedbackItem[]; csrf_token: string };

export type SurveyRow = {
  ts: number;
  name?: string | null;
  discord_id?: string | null;
  client?: string | null;
  understand?: string | null; // 已下线的「限额理解」题：只有旧回答有值，后台不再展示
  improve?: string | null;
  improve_other?: string | null;
};
export type SurveyResp = {
  rows: SurveyRow[];
};
