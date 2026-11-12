//! Image model policy, request normalization, reference validation and cost estimation.
//! Functions use JSON values and native validation errors; random values are supplied by callers.

use crate::validation::{
    self, Obj, ValidationError, ValidationResult, enabled_opt, int_like, int_or, json_int,
    len_or_empty, nd_class, num_eq_int, space_class, str_of_get, strict_int, text,
};
use crate::{primitives, values};
use fancy_regex::Regex as FancyRegex;
use regex::Regex;
use serde_json::Value;
use std::sync::OnceLock;

// ----------------------------------------------------------------------------- 常量

pub const V5_COST_MULTIPLIER: f64 = 1.5;

/// 仅允许明确认识的图片模型族；未知模型不能当旧模型，否则上游新增模型会绕过 V5 / Anlas 保护。
/// V3 / V2 / V1 时代模型 10-31 起整体下线（站长：没人用了），不再列入白名单——请求会得到
/// 「本站不支持该模型」，历史日志里的显示标签保留在后台前端。
pub const LEGACY_IMAGE_MODEL_PREFIXES: [&str; 2] = ["nai-diffusion-4-5", "nai-diffusion-4"];

/// 免费 Key 按预设钳制尺寸。
pub const V5_NORMAL_PRESETS: [(i64, i64); 3] = [(832, 1216), (1216, 832), (1024, 1024)];

/// V4 / V4.5 传预编码 Vibe，V5 没有 Vibe（只有精确参考）。
pub const VIBE_ENCODED_MODELS: [&str; 11] = [
    "nai-diffusion-4",
    "nai-diffusion-4-full",
    "nai-diffusion-4-full-inpainting",
    "nai-diffusion-4-curated",
    "nai-diffusion-4-curated-preview",
    "nai-diffusion-4-curated-inpainting",
    "nai-diffusion-4-5",
    "nai-diffusion-4-5-full",
    "nai-diffusion-4-5-full-inpainting",
    "nai-diffusion-4-5-curated",
    "nai-diffusion-4-5-curated-inpainting",
];
/// `{m for m in VIBE_ENCODED_MODELS if m.startswith("nai-diffusion-4-5")}`
pub const PRECISE_REFERENCE_MODELS: [&str; 5] = [
    "nai-diffusion-4-5",
    "nai-diffusion-4-5-full",
    "nai-diffusion-4-5-full-inpainting",
    "nai-diffusion-4-5-curated",
    "nai-diffusion-4-5-curated-inpainting",
];
pub const REFERENCE_LIMIT: usize = 16;
pub const VIBE_ENCODING_ANLAS: i64 = 2;
pub const REFERENCE_FIELDS: [&str; 9] = [
    "reference_image_multiple",
    "reference_image_multiple_cached",
    "reference_information_extracted_multiple",
    "reference_strength_multiple",
    "director_reference_images_cached",
    "director_reference_descriptions",
    "director_reference_information_extracted",
    "director_reference_strength_values",
    "director_reference_secondary_strength_values",
];

pub const INTEGER_IMAGE_PARAMS: [&str; 4] = ["width", "height", "steps", "n_samples"];

/// 上游实测限制：提示词上限 51200（按 UTF-8 字节）、文生图宽高必须是 64 的整数倍、
/// V4 / V4.5 / V5 用下列采样器上游直接 500。
pub const MAX_PROMPT_BYTES: usize = 51200;

// ----------------------------------------------------------------------------- 尺寸（唯一的定义处）
//
// 所有和图片尺寸有关的数字都只在这里定义一次，别处一律引用。以前 `1024 * 1024`、
// `1_011_712`、`8192`、`3_145_728` 分散在 policy / media / pricing 里各写一遍，
// 改一处漏一处；现在要改哪条规则，改这里。

/// 上游要求：文生图的宽高必须是这个数的整数倍。`fit_size` 和三个 V5 预设都必须落在它上面。
pub const DIMENSION_STEP: i64 = 64;

/// 免费档的单张面积上限（= 1024×1024）。免费 Key 的生成请求按它等比缩小。
pub const FREE_MAX_PIXELS: i64 = 1024 * 1024;

/// 图片工具（放大 / 导演工具）允许的输入面积上限，是生成档的 3 倍（= 1536×2048）。
pub const TOOL_MAX_PIXELS: u64 = 3_145_728;

/// 任何一边的像素上限（解码与校验用）。
pub const MAX_SIDE: u32 = 8192;

/// 「连接测试」请求的面积线（512×512）：步数、面积、提示词长度三条同时满足才算测试请求，
/// 见 `probe::is_connection_test`。依据是线上 7 天没有任何正常请求同时满足这三条。
pub const TEST_REQUEST_MAX_PIXELS: i64 = 512 * 512;

/// 两个长方形 V5 预设的面积（832×1216 = 1_011_712，也是三个预设里最小的）。
/// 图片工具把「比这还小的图」先放大到 [`FREE_MAX_PIXELS`] 再处理，与官方网页端一致。
pub const NORMAL_PIXELS: u64 = 832 * 1216;
pub const V4_V5_REJECTED_SAMPLERS: [&str; 8] = [
    "plms",
    "ddim",
    "ddim_v3",
    "k_dpm_adaptive",
    "k_dpm_fast",
    "k_dpmpp_3m_sde",
    "nai_smea",
    "nai_smea_dyn",
];
/// 只有 V5 不支持的采样器（V4.5 能用）：线上 V5 配 k_dpm_2 每次都是上游 500。
pub const V5_REJECTED_SAMPLERS: [&str; 1] = ["k_dpm_2"];
/// NovelAI V4 / V4.5 角色提示词上限。
pub const MAX_CHARACTERS: usize = 6;
/// NovelAI V5 Full / Curated 角色提示词上限。
pub const MAX_V5_CHARACTERS: usize = 22;

/// V5 Medium 档（蒸馏模型，锁 14 步 / Euler-a / Heavy 负面预设）。
pub const V5_MEDIUM_OF: [(&str, &str); 2] = [
    ("nai-diffusion-5-full", "nai-diffusion-5-full-medium"),
    (
        "nai-diffusion-5-full-inpainting",
        "nai-diffusion-5-full-medium-inpainting",
    ),
];
pub const V5_MEDIUM_STEPS: i64 = 14;
pub const V5_MEDIUM_SAMPLER: &str = "k_euler_ancestral";
pub const V5_MEDIUM_UC_PRESET: i64 = 0;
pub const V5_MEDIUM_UC: &str = "lowres, artistic error, film grain, scan artifacts, worst quality, bad quality, jpeg artifacts, \
very displeasing, chromatic aberration, dithering, halftone, screentone, multiple views, logo, \
too many watermarks, negative space, blank page";

const PNG_MAGIC: &[u8] = b"\x89PNG\r\n\x1a\n";

mod params;
mod pricing;
mod probe;
mod prompts;
mod references;
mod share;

pub use params::*;
pub use pricing::*;
pub use probe::*;
pub use prompts::*;
pub use references::*;
pub use share::{
    adapt_share_image_request, apply_share_unknown_retry, share_unknown_fields, strip_named_fields,
};
