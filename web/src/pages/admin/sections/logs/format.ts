/** 日志相关的显示格式化（纯函数）：模型短名、功能名、耗时。 */
import { secondsText } from "../../../../lib/format";
import { FEATURES, RETIRED } from "../features";

/** 后端登记的定时任务 name → 标题（Bug 来源 `job:<name>` 用它翻译） */
const JOB_TITLES: Record<string, string> = {
  quota: "额度同步",
  upstream_snapshot: "上游 V5 快照",
  hourly_cap: "每小时上限自动调整",
  breaker: "全站熔断",
  key_guard: "单 Key 限流",
  maintenance: "维护",
  abuse_scan: "小号扫描",
  anlas_reconcile: "Anlas 自动核对",
  idle_recycle: "闲置回收",
};

/** 请求日志 kind → 功能 id（后端 features.KIND_FEATURE 的副本） */
const KIND_FEATURE: Record<string, string> = {
  image: "image",
  image_stream: "image",
  upscale: "upscale",
  "augment-image": "augment",
  vibe_encode: "vibe",
  tags: "tags",
  text: "text",
  chat: "text",
  voice: "voice",
};

export function kindLabel(kind: string | null | undefined): string {
  const id = KIND_FEATURE[kind ?? ""];
  const f = FEATURES.find((x) => x.id === id) ?? RETIRED.find((x) => x.id === id);
  return f ? f.short : kind || "—";
}

/** 模型短名：nai-diffusion-4-5-full → V4.5（完整名放 title） */
export function shortModel(model: string | null | undefined): string {
  const m = String(model ?? "");
  if (!m) return "";
  const base = /diffusion-5/.test(m)
    ? "V5"
    : /diffusion-4-5/.test(m)
      ? "V4.5"
      : /diffusion-4/.test(m)
        ? "V4"
        : /furry-3/.test(m)
          ? "Furry V3"
          : /diffusion-3/.test(m)
            ? "V3"
            : /diffusion-2/.test(m)
              ? "V2"
              : /furry/.test(m)
                ? "Furry"
                : /^nai-diffusion$/.test(m)
                  ? "V1"
                  : "";
  if (!base) return m.length > 18 ? `${m.slice(0, 16)}…` : m;
  return base + (/curated/.test(m) ? " Curated" : "") + (/inpainting/.test(m) ? " 重绘" : "");
}

/** 耗时：生成 4.2 秒、等 12 秒（等待 < 100ms 不显示） */
export function timing(waitMs: number, durMs: number): { gen: string; wait: string } {
  return { gen: durMs ? secondsText(durMs) : "", wait: waitMs >= 100 ? secondsText(waitMs) : "" };
}

/** Bug 来源 → 中文 */
const BUG_SRC: Record<string, string> = {
  request: "接口异常",
  upstream: "上游故障",
  gate: "网关故障",
  accounting: "记账写入",
  disconnect: "客户端断开",
  audit: "生成记录",
  key_sources: "来源统计",
  quota_algo: "动态额度",
  audit_image_purge: "原图清理",
  touch_key: "活跃心跳",
  guard_seed: "账号保护",
  banned_link: "封禁号关联",
};
const MAINT_TASKS: Record<string, string> = {
  disk: "磁盘监控",
  purge: "历史清理",
  quota: "额度重置",
  bug_triage: "Bug 自动处理",
  official: "官方状态",
  sync_roles: "同步身份组",
  backfill_profiles: "补全资料",
  sweep_departed: "退群回收",
};
const WEB_PAGES: Record<string, string> = {
  admin: "后台",
  landing: "首页",
};
export function bugSource(s: string): string {
  if (BUG_SRC[s]) return BUG_SRC[s];
  if (s.startsWith("maintenance:")) return `维护任务 · ${MAINT_TASKS[s.slice(12)] ?? s.slice(12)}`;
  if (s.startsWith("web:")) return `网页脚本 · ${WEB_PAGES[s.slice(4)] ?? s.slice(4)}`;
  if (s.startsWith("job:")) return `定时任务 · ${JOB_TITLES[s.slice(4)] ?? s.slice(4)}`;
  return s;
}

/** Bug 自动处理的原因码（gate_core::errors::AutoReason::code）→ 页面上显示的原因 */
const AUTO_REASON: Record<string, string> = {
  network: "网络断开，不算 Bug",
  upstream: "上游故障，不算 Bug",
  foreign_script: "非本站脚本",
  stale_warn: "24 小时未再出现",
  stale_error: "7 天未再出现",
};
export function autoReason(code: string): string {
  return AUTO_REASON[code] ?? "自动处理";
}
