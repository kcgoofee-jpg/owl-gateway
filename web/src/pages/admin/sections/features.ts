/** 日志筛选把文生图按模型拆开；历史功能只给旧日志显示。正在用的功能名来自 GET /admin/api/features。 */

export type Feature = { id: string; label: string; short: string };
export type FeatureName = { id: string; label: string };

/** 短名取完整名里括号前的部分：「导演工具（线稿、上色等）」→「导演工具」。 */
export function featureShort(label: string): string {
  return label.split("（")[0] || label;
}

export function asFeatures(rows: FeatureName[]): Feature[] {
  return rows.map((row) => ({ id: row.id, label: row.label, short: featureShort(row.label) }));
}

/** 日志筛选专用：文生图按记录中的模型版本拆开，其余功能用接口给的名字。 */
export function logFeatures(features: Feature[]): Feature[] {
  return [
    { id: "image", label: "文生图（全部版本）", short: "文生图（全部）" },
    { id: "image_v45", label: "文生图 V4.5", short: "文生图 V4.5" },
    { id: "image_v5", label: "文生图 V5", short: "文生图 V5" },
    ...features.filter((f) => f.id !== "image"),
  ];
}

/** 已删除的功能（3.0 起不再提供），只用来给历史日志显示名字，不进筛选 */
export const RETIRED: Feature[] = [
  { id: "text", label: "文本生成 / OpenAI 兼容聊天", short: "文本 / 聊天" },
  { id: "voice", label: "语音合成", short: "语音合成" },
];
