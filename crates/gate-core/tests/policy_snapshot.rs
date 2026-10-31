//! `policy`（出图请求的规范化、计费估算、参数钳制、提示词清洗、参考图与 Vibe 校验、免费资格、Key 生成和打码）的快照。
//!
//! 每个函数一份 `policy/<函数名>`。请求体类函数同时记返回值和（原地修改后的）入参，键顺序也要一致；
//! 抛异常的记 `{"err": 类型, "msg": 消息}`。常量和字符类是手写的表。

mod common;

use common::{bytes_of, outcome};
use gate_core::policy::{self, ImageCost};
use gate_core::primitives;
use gate_core::validation::{is_digit, is_nd, json_int};
use gate_snapshot::snapshot::check;
use gate_snapshot::{Case, f};
use serde_json::{Value, json};

const M: &str = env!("CARGO_MANIFEST_DIR");

/// 测试里 `random.randint(0, 4294967295)` 固定返回这个值。
const FIXED_RANDINT: u32 = 2_718_281_828;

fn opt_str(s: Option<String>) -> Value {
    s.map_or(Value::Null, Value::String)
}

fn strs(v: &[String]) -> Value {
    Value::Array(v.iter().cloned().map(Value::String).collect())
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or_else(|| panic!("期望字符串：{v}"))
}

fn int(v: &Value) -> i128 {
    gate_core::validation::strict_int(v).unwrap_or_else(|| panic!("期望整数：{v}"))
}

fn kw_bool(c: &Case, name: &str, default: bool) -> bool {
    c.kwarg(name).and_then(Value::as_bool).unwrap_or(default)
}

fn kw_i64(c: &Case, name: &str) -> Option<i64> {
    c.kwarg(name).and_then(Value::as_i64)
}

fn cost_json(r: ImageCost) -> Value {
    json!({"anlas": r.anlas, "v5": r.v5})
}

fn pair(a: i128, b: i128) -> Value {
    json!([json_int(a), json_int(b)])
}

/// 跑一份 `policy/<name>` 快照；`min` 是至少要有的用例数（防止快照被误删成空壳）。
#[track_caller]
fn run(name: &str, min: usize, mut f: impl FnMut(&Case) -> Value) {
    let mut n = 0;
    check(M, &format!("policy/{name}"), |c| {
        n += 1;
        Some(f(c))
    });
    assert!(n >= min, "policy/{name} 用例太少：{n}");
}

// ----------------------------------------------------------------------------- 常量

#[test]
fn constants() {
    assert_eq!(policy::V5_COST_MULTIPLIER, 1.5);
    // V3 / V2 / V1 时代模型 10-31 起下线：白名单里不再有它们
    assert_eq!(
        policy::LEGACY_IMAGE_MODEL_PREFIXES,
        ["nai-diffusion-4-5", "nai-diffusion-4"]
    );
    let sorted = |xs: &[&str]| -> Vec<String> {
        let mut v: Vec<String> = xs.iter().map(|x| x.to_string()).collect();
        v.sort();
        v
    };
    assert_eq!(
        policy::V5_NORMAL_PRESETS,
        [(832, 1216), (1216, 832), (1024, 1024)]
    );
    assert_eq!(
        sorted(&policy::VIBE_ENCODED_MODELS),
        [
            "nai-diffusion-4",
            "nai-diffusion-4-5",
            "nai-diffusion-4-5-curated",
            "nai-diffusion-4-5-curated-inpainting",
            "nai-diffusion-4-5-full",
            "nai-diffusion-4-5-full-inpainting",
            "nai-diffusion-4-curated",
            "nai-diffusion-4-curated-inpainting",
            "nai-diffusion-4-curated-preview",
            "nai-diffusion-4-full",
            "nai-diffusion-4-full-inpainting"
        ]
    );
    assert_eq!(
        sorted(&policy::PRECISE_REFERENCE_MODELS),
        [
            "nai-diffusion-4-5",
            "nai-diffusion-4-5-curated",
            "nai-diffusion-4-5-curated-inpainting",
            "nai-diffusion-4-5-full",
            "nai-diffusion-4-5-full-inpainting"
        ]
    );
    assert_eq!(policy::REFERENCE_LIMIT, 16);
    assert_eq!(policy::VIBE_ENCODING_ANLAS, 2);
    assert_eq!(
        policy::REFERENCE_FIELDS,
        [
            "reference_image_multiple",
            "reference_image_multiple_cached",
            "reference_information_extracted_multiple",
            "reference_strength_multiple",
            "director_reference_images_cached",
            "director_reference_descriptions",
            "director_reference_information_extracted",
            "director_reference_strength_values",
            "director_reference_secondary_strength_values"
        ]
    );
    assert_eq!(
        policy::INTEGER_IMAGE_PARAMS,
        ["width", "height", "steps", "n_samples"]
    );
    assert_eq!(policy::MAX_PROMPT_BYTES, 51200);
    assert_eq!(policy::DIMENSION_STEP, 64);
    assert_eq!(
        sorted(&policy::V4_V5_REJECTED_SAMPLERS),
        [
            "ddim",
            "ddim_v3",
            "k_dpm_adaptive",
            "k_dpm_fast",
            "k_dpmpp_3m_sde",
            "nai_smea",
            "nai_smea_dyn",
            "plms"
        ]
    );
    assert_eq!(policy::MAX_CHARACTERS, 6);
    assert_eq!(
        policy::V5_MEDIUM_OF,
        [
            ("nai-diffusion-5-full", "nai-diffusion-5-full-medium"),
            (
                "nai-diffusion-5-full-inpainting",
                "nai-diffusion-5-full-medium-inpainting"
            )
        ]
    );
    assert_eq!(policy::V5_MEDIUM_STEPS, 14);
    assert_eq!(policy::V5_MEDIUM_SAMPLER, "k_euler_ancestral");
    assert_eq!(policy::V5_MEDIUM_UC_PRESET, 0);
    assert_eq!(
        policy::V5_MEDIUM_UC,
        "lowres, artistic error, film grain, scan artifacts, worst quality, bad quality, \
         jpeg artifacts, very displeasing, chromatic aberration, dithering, halftone, \
         screentone, multiple views, logo, too many watermarks, negative space, blank page"
    );
}

#[test]
fn numeric_fields_use_ascii_digits() {
    assert!(is_nd('5') && is_digit('0'));
    assert!(!is_nd('５') && !is_digit('²'));
    assert!(primitives::is_space('\u{3000}'));
}

// ----------------------------------------------------------------------------- 小工具

#[test]
fn base64_data_snapshot() {
    run("base64_data", 20, |c| {
        json!(policy::base64_data(
            Some(c.arg(0)),
            kw_bool(c, "source_image", false),
            kw_bool(c, "png", false),
        ))
    });
}

#[test]
fn unit_value_snapshot() {
    run("unit_value", 20, |c| json!(policy::unit_value(c.arg(0))));
}

#[test]
fn reference_list_snapshot() {
    run("reference_list", 5, |c| {
        let p = c.arg(0).as_object().unwrap();
        outcome(policy::reference_list(p, s(c.arg(1))).map(|l| Value::Array(l.to_vec())))
    });
}

#[test]
fn validate_vibe_encoding_snapshot() {
    run("validate_vibe_encoding", 20, |c| {
        opt_str(policy::validate_vibe_encoding(c.arg(0)))
    });
}

#[test]
fn validate_image_references_snapshot() {
    run("validate_image_references", 20, |c| {
        opt_str(policy::validate_image_references(c.arg(0)))
    });
}

#[test]
fn reference_surcharge_snapshot() {
    run("reference_surcharge", 20, |c| {
        outcome(policy::reference_surcharge(c.arg(0)).map(|n| json!(n)))
    });
}

// ----------------------------------------------------------------------------- 规范化与提示词

#[test]
fn normalize_image_request_snapshot() {
    run("normalize_image_request", 20, |c| {
        let mut body = c.arg(0).clone();
        match policy::normalize_image_request(&mut body, &mut || FIXED_RANDINT) {
            Ok(()) => json!({"body": body}),
            Err(e) => json!({"err": e.type_name(), "msg": e.message(), "body": body}),
        }
    });
}

#[test]
fn prompt_texts_snapshot() {
    run("prompt_texts", 20, |c| {
        let items: Vec<Value> = policy::prompt_texts(c.arg(0))
            .into_iter()
            .map(|(path, v)| json!([path, v.cloned().unwrap_or(Value::Null)]))
            .collect();
        Value::Array(items)
    });
}

#[test]
fn fix_name_digit_weights_snapshot() {
    run("fix_name_digit_weights", 20, |c| {
        let mut body = c.arg(0).clone();
        let n = policy::fix_name_digit_weights(&mut body);
        json!({"n": n, "body": body})
    });
}

#[test]
fn clean_prompt_syntax_snapshot() {
    run("clean_prompt_syntax", 20, |c| {
        json!(policy::clean_prompt_syntax(s(c.arg(0))))
    });
}

#[test]
fn sanitize_and_translate_prompts_snapshot() {
    run("sanitize_and_translate_prompts", 20, |c| {
        let mut body = c.arg(0).clone();
        let n = policy::sanitize_and_translate_prompts(&mut body);
        json!({"n": n, "body": body})
    });
}

#[test]
fn upstream_parameter_problem_snapshot() {
    run("upstream_parameter_problem", 20, |c| {
        opt_str(policy::upstream_parameter_problem(c.arg(0)))
    });
}

// ----------------------------------------------------------------------------- 模型 / 计费

/// `(model or "")`：None 表示空串。
fn model_arg(c: &Case) -> &str {
    c.arg(0).as_str().unwrap_or("")
}

#[test]
fn is_v5_model_snapshot() {
    run("is_v5_model", 20, |c| {
        json!(policy::is_v5_model(model_arg(c)))
    });
}

#[test]
fn image_model_tier_snapshot() {
    run("image_model_tier", 20, |c| {
        policy::image_model_tier(model_arg(c)).map_or(Value::Null, |t| json!(t))
    });
}

#[test]
fn is_v5_medium_snapshot() {
    run("is_v5_medium", 10, |c| {
        json!(policy::is_v5_medium(model_arg(c)))
    });
}

#[test]
fn per_image_cost_snapshot() {
    run("per_image_cost", 20, |c| {
        outcome(
            policy::per_image_cost(
                int(c.arg(0)),
                int(c.arg(1)),
                int(c.arg(2)),
                c.arg(3).as_bool().unwrap(),
                c.arg(4).as_bool().unwrap(),
            )
            .map(f),
        )
    });
}

#[test]
fn has_paid_extras_snapshot() {
    run("has_paid_extras", 20, |c| {
        outcome(policy::has_paid_extras(c.arg(0)).map(|b| json!(b)))
    });
}

#[test]
fn opus_free_eligible_snapshot() {
    run("opus_free_eligible", 20, |c| {
        outcome(policy::opus_free_eligible(c.arg(0)).map(|b| json!(b)))
    });
}

#[test]
fn snap_v5_preset_snapshot() {
    run("snap_v5_preset", 20, |c| {
        let (w, h) = policy::snap_v5_preset(int(c.arg(0)), int(c.arg(1)));
        pair(w, h)
    });
}

#[test]
fn estimate_image_cost_snapshot() {
    run("estimate_image_cost", 20, |c| {
        outcome(
            policy::estimate_image_cost(
                c.arg(0),
                kw_bool(c, "is_opus", true),
                kw_bool(c, "v5_allowance_available", true),
            )
            .map(cost_json),
        )
    });
}

// ----------------------------------------------------------------------------- 钳制

#[test]
fn fit_size_snapshot() {
    run("fit_size", 20, |c| {
        outcome(
            policy::fit_size(int(c.arg(0)), int(c.arg(1)), int(c.arg(2))).map(|(w, h)| pair(w, h)),
        )
    });
}

#[test]
fn clamp_image_params_snapshot() {
    run("clamp_image_params", 20, |c| {
        outcome(
            policy::clamp_image_params(
                c.arg(0),
                kw_i64(c, "max_pixels").unwrap(),
                kw_i64(c, "max_steps").unwrap(),
                kw_bool(c, "allow_img2img", false),
            )
            .map(|(out, notes, err)| json!([out, strs(&notes), opt_str(err)])),
        )
    });
}

#[test]
fn fill_v4_prompt_snapshot() {
    run("fill_v4_prompt", 20, |c| {
        let (out, notes) = policy::fill_v4_prompt(c.arg(0));
        json!([out, strs(&notes)])
    });
}

#[test]
fn medium_normalize_snapshot() {
    run("medium_normalize", 20, |c| {
        let (out, notes) = policy::medium_normalize(c.arg(0));
        json!([out, strs(&notes)])
    });
}

#[test]
fn guard_image_action_snapshot() {
    run("guard_image_action", 20, |c| {
        let (out, notes, err) = policy::guard_image_action(c.arg(0));
        json!([out, strs(&notes), opt_str(err)])
    });
}

// ----------------------------------------------------------------------------- 杂项

#[test]
fn mask_token_snapshot() {
    run("mask_token", 20, |c| {
        let keep = c.args().get(1).map_or(6, |k| int(k) as usize);
        json!(policy::mask_token(s(c.arg(0)), keep))
    });
}

#[test]
fn gen_key_snapshot() {
    run("gen_key", 20, |c| {
        let bytes = bytes_of(c.arg(1));
        let arr: [u8; 24] = bytes.try_into().expect("24 字节");
        json!(policy::gen_key(s(c.arg(0)), &arr))
    });
}
