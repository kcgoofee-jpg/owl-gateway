//! 功能名表。
//!
//! 只负责「功能叫什么、对应哪几种 `usage_log.kind`」这类**字典**，不再有准入判定——
//! 功能开不开放只看成员规则（`MemberPolicy` 里的 `image / v5 / vibe / tags / upscale / augment /
//! img2img / paid_parameters`），站点级的 `features_global` 开关已删除。

/// 功能名 → 中文说明（顺序有意义：后台按它排列）。
pub const FEATURES: [(&str, &str); 5] = [
    ("image", "文生图"),
    ("upscale", "放大"),
    ("augment", "导演工具（线稿、上色等）"),
    ("vibe", "Vibe 编码"),
    ("tags", "标签补全"),
];

/// 成员规则里的功能开关（含不记入 `usage_log.kind` 的项）。公开状态、`/v1/me`、领 Key 文案用这份。
pub const MEMBER_FEATURES: [(&str, &str); 8] = [
    ("image", "文生图"),
    ("v5", "V5"),
    ("vibe", "Vibe 编码"),
    ("tags", "标签补全"),
    ("img2img", "图生图与局部重绘"),
    ("upscale", "放大"),
    ("augment", "导演工具（线稿、上色等）"),
    ("paid_parameters", "高规格生图"),
];

/// 需要消耗 Anlas 的功能：本站只提供免费出图，拒绝时说明原因，免得成员以为是 Key 坏了。
pub const ANLAS_FEATURES: [(&str, &str); 3] = [
    ("vibe", "每次编码参考图约 2 Anlas"),
    ("upscale", "按图片大小扣 Anlas"),
    ("augment", "按图片大小扣 Anlas"),
];

/// 用量日志 `usage_log.kind` → 功能。后台按功能统计与筛选日志时使用。
pub const KIND_FEATURE: [(&str, &str); 6] = [
    ("image", "image"),
    ("image_stream", "image"),
    ("upscale", "upscale"),
    ("augment-image", "augment"),
    ("vibe_encode", "vibe"),
    ("tags", "tags"),
];

/// `kinds_for(feature)`：该功能对应的所有 `usage_log.kind`（按 `KIND_FEATURE` 的顺序）。
pub fn kinds_for(feature: &str) -> Vec<&'static str> {
    KIND_FEATURE
        .iter()
        .filter(|(_, f)| *f == feature)
        .map(|(k, _)| *k)
        .collect()
}
