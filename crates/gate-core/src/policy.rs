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
pub const LEGACY_IMAGE_MODEL_PREFIXES: [&str; 5] = [
    "nai-diffusion-4-5",
    "nai-diffusion-4",
    "nai-diffusion-3",
    "nai-diffusion-2",
    "nai-diffusion-furry-3",
];
pub const LEGACY_IMAGE_MODEL_EXACT: [&str; 3] =
    ["safe-diffusion", "nai-diffusion", "nai-diffusion-furry"];

/// 免费 Key 按预设钳制尺寸。
pub const V5_NORMAL_PRESETS: [(i64, i64); 3] = [(832, 1216), (1216, 832), (1024, 1024)];

/// V3 传原图，V4 / V4.5 传预编码 Vibe，V5 两者都没有。
pub const VIBE_RAW_MODELS: [&str; 4] = [
    "nai-diffusion-3",
    "nai-diffusion-3-inpainting",
    "nai-diffusion-furry-3",
    "nai-diffusion-furry-3-inpainting",
];
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
pub const DIMENSION_STEP: i64 = 64;
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

pub use params::*;
pub use pricing::*;
pub use probe::*;
pub use prompts::*;
pub use references::*;
