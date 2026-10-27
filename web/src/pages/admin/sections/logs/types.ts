/** 日志相关接口的数据形状。 */

export type Paged = { page: number; per_page: number; total: number; pages: number };

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
