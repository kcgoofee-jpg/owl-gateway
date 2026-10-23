/** 功能表（后端 features.FEATURES 的副本）：后台里所有显示功能名的地方都从这里取。 */

type Feature = { id: string; label: string; short: string };

/** 功能 id → 完整名 / 短名 */
export const FEATURES: Feature[] = [
  { id: "image", label: "文生图", short: "文生图" },
  { id: "upscale", label: "放大", short: "放大" },
  { id: "augment", label: "导演工具（线稿、上色等）", short: "导演工具" },
  { id: "vibe", label: "Vibe 编码", short: "Vibe 编码" },
  { id: "tags", label: "标签补全", short: "标签补全" },
];

/** 已删除的功能（3.0 起不再提供），只用来给历史日志显示名字，不进筛选 */
export const RETIRED: Feature[] = [
  { id: "text", label: "文本生成 / OpenAI 兼容聊天", short: "文本 / 聊天" },
  { id: "voice", label: "语音合成", short: "语音合成" },
];
