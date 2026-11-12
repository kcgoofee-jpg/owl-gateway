//! 拼车网关（NovelAI Shared Gateway）入站校验比官网严：多一个字段、空数组、0/1 当布尔都会 422。
//! 发出前按对方公开实现收一收；对方点名的未知字段可以再剥一次后重发。

use super::*;
use serde_json::{Map, Value};

const SHARE_BODY_KEYS: &[&str] = &[
    "input",
    "model",
    "action",
    "parameters",
    "use_new_shared_trial",
    "allowPurchasedAnlas",
];
const BOOLEAN_PARAMETERS: &[&str] = &[
    "qualityToggle",
    "dynamic_thresholding",
    "legacy",
    "legacy_uc",
    "legacy_v3_extend",
    "add_original_image",
    "autoSmea",
    "sm",
    "sm_dyn",
    "deliberate_euler_ancestral_bug",
    "prefer_brownian",
    "use_coords",
    "normalize_reference_strength_multiple",
    "color_correct",
    "straight_alpha",
    "tag_hint_transparent_background",
    "explike_fine_detail",
    "minimize_sigma_inf",
    "uncond_per_vibe",
    "wonky_vibe_correlation",
];
const DROP_IF_INACTIVE: &[&str] = &[
    "controlnet_model",
    "controlnet_condition",
    "characterRef",
    "upscale",
    "upscaled_enhance",
    "characterReferences",
];
const DROP_IF_EMPTY: &[&str] = &["lora_unet_weights", "lora_clip_weights"];
const NULL_ALSO_DROP: &[&str] = &[
    "uncond_scale",
    "dynamic_thresholding_percentile",
    "dynamic_thresholding_mimic_scale",
    "cfg_sched_eligibility",
    "version",
];

/// 发给拼车网关前整理：去掉对方会 422 的官网多余顶层字段、空扩展、非 V3 的 SMEA，并把 0/1 收成布尔。
pub fn adapt_share_image_request(body: &Value) -> Value {
    let Some(src) = body.as_object() else {
        return body.clone();
    };
    let mut out = Map::new();
    for key in SHARE_BODY_KEYS {
        if let Some(v) = src.get(*key) {
            out.insert((*key).to_string(), v.clone());
        }
    }
    if let Some(Value::Object(p0)) = out.get("parameters").cloned() {
        out.insert(
            "parameters".into(),
            Value::Object(adapt_share_parameters(&p0, model_of(&out))),
        );
    }
    Value::Object(out)
}

/// `未知字段：foo、bar。` → `["foo","bar"]`。对方最多回 6 个名字，异常名字写成「（字段名已省略）」。
pub fn share_unknown_fields(message: &str) -> Vec<String> {
    let Some(rest) = message.split("未知字段：").nth(1) else {
        return Vec::new();
    };
    let rest = rest
        .trim()
        .trim_end_matches('。')
        .trim_end_matches('等')
        .trim();
    rest.split('、')
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != "（字段名已省略）")
        .filter(|s| {
            let b = s.as_bytes();
            (1..=64).contains(&b.len())
                && b[0].is_ascii_alphabetic()
                && b.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'_')
        })
        .map(str::to_string)
        .collect()
}

/// 对方 422 点名的未知字段：从当前待发正文里剥掉。剥到了返回 true，调用方再发一次。
pub fn apply_share_unknown_retry(
    share_body: &mut Option<Value>,
    original: Option<&Value>,
    message: &str,
) -> bool {
    let fields = share_unknown_fields(message);
    if fields.is_empty() {
        return false;
    }
    let mut adapted = share_body
        .take()
        .or_else(|| original.cloned())
        .unwrap_or(Value::Null);
    let changed = strip_named_fields(&mut adapted, &fields);
    *share_body = Some(adapted);
    changed
}

/// 从顶层和 `parameters` 里删掉这些键。删到了返回 true。
pub fn strip_named_fields(body: &mut Value, names: &[String]) -> bool {
    if names.is_empty() {
        return false;
    }
    let Some(obj) = body.as_object_mut() else {
        return false;
    };
    let mut changed = false;
    for name in names {
        changed |= obj.shift_remove(name.as_str()).is_some();
    }
    if let Some(Value::Object(p)) = obj.get_mut("parameters") {
        for name in names {
            changed |= p.shift_remove(name.as_str()).is_some();
        }
    }
    changed
}

fn model_of(obj: &Map<String, Value>) -> String {
    obj.get("model")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn adapt_share_parameters(p0: &Map<String, Value>, model: String) -> Map<String, Value> {
    let mut p = p0.clone();
    for (alias, field) in [
        ("uc_preset", "ucPreset"),
        ("quality_toggle", "qualityToggle"),
    ] {
        let Some(alias_val) = p.remove(alias) else {
            continue;
        };
        if !p.contains_key(field) {
            p.insert(field.into(), alias_val);
        }
    }
    for field in DROP_IF_INACTIVE {
        if p.get(*field).is_none_or(|v| !values::enabled(v)) {
            p.remove(*field);
        }
    }
    for field in DROP_IF_EMPTY {
        if p.get(*field).is_none_or(|v| !values::enabled(v)) {
            p.remove(*field);
        }
    }
    for field in BOOLEAN_PARAMETERS.iter().chain(NULL_ALSO_DROP) {
        if matches!(p.get(*field), Some(Value::Null)) {
            p.remove(*field);
        }
    }
    for field in BOOLEAN_PARAMETERS {
        if let Some(coerced) = p.get(*field).and_then(as_bool) {
            p.insert((*field).into(), Value::Bool(coerced));
        }
    }
    if p.contains_key("autoSmea") {
        p.insert("autoSmea".into(), Value::Bool(false));
    }
    if !model.ends_with("-3") {
        if p.get("sm").is_some() {
            p.insert("sm".into(), Value::Bool(false));
        }
        if p.get("sm_dyn").is_some() {
            p.insert("sm_dyn".into(), Value::Bool(false));
        }
    }
    if let Some(fmt) = p.get("image_format").and_then(Value::as_str)
        && fmt != "png"
    {
        p.insert("image_format".into(), Value::String("png".into()));
    }
    if let Some(v) = p.get("negative_prompt")
        && !v.is_string()
    {
        p.insert(
            "negative_prompt".into(),
            Value::String(str_or_empty(Some(v))),
        );
    }
    p
}

fn as_bool(v: &Value) -> Option<bool> {
    match v {
        Value::Bool(b) => Some(*b),
        Value::Number(n) => n.as_f64().and_then(|x| {
            if x == 0.0 {
                Some(false)
            } else if x == 1.0 {
                Some(true)
            } else {
                None
            }
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn drops_website_extras_that_make_share_gateways_422() {
        let body = json!({
            "input": "1girl",
            "model": "nai-diffusion-4-5-full",
            "action": "generate",
            "website_extra": true,
            "parameters": {
                "width": 832,
                "characterReferences": [],
                "autoSmea": true,
                "sm": true,
                "qualityToggle": 1,
                "uc_preset": 0,
                "image_format": "webp",
                "controlnet_model": "",
                "lora_unet_weights": {},
            }
        });
        let out = adapt_share_image_request(&body);
        assert!(out.get("website_extra").is_none());
        let p = out["parameters"].as_object().unwrap();
        assert!(p.get("characterReferences").is_none());
        assert_eq!(p["autoSmea"], json!(false));
        assert_eq!(p["sm"], json!(false));
        assert_eq!(p["qualityToggle"], json!(true));
        assert_eq!(p["ucPreset"], json!(0));
        assert!(p.get("uc_preset").is_none());
        assert_eq!(p["image_format"], json!("png"));
        assert!(p.get("controlnet_model").is_none());
        assert!(p.get("lora_unet_weights").is_none());
        assert_eq!(p["width"], json!(832));
    }

    #[test]
    fn coerces_non_string_negative_prompt_for_share() {
        let body = json!({
            "input": "1girl",
            "model": "nai-diffusion-4-5-full",
            "parameters": {"negative_prompt": 0.5, "width": 832}
        });
        let out = adapt_share_image_request(&body);
        assert_eq!(out["parameters"]["negative_prompt"], json!("0.5"));
        let already = json!({
            "parameters": {"negative_prompt": "lowres"}
        });
        let kept = adapt_share_image_request(&already);
        assert_eq!(kept["parameters"]["negative_prompt"], json!("lowres"));
    }

    #[test]
    fn keeps_active_character_references_for_production_share() {
        let body = json!({
            "model": "nai-diffusion-4-5-full",
            "parameters": {"characterReferences": ["abc"]}
        });
        let out = adapt_share_image_request(&body);
        assert_eq!(out["parameters"]["characterReferences"], json!(["abc"]));
    }

    #[test]
    fn parses_and_strips_unknown_field_names() {
        let msg = "包含尚未支持的生成参数；请关闭 ControlNet、自动放大或其他未支持的扩展。 未知字段：foo、bar。";
        assert_eq!(share_unknown_fields(msg), vec!["foo", "bar"]);
        let mut body = json!({"foo": 1, "parameters": {"bar": 2, "width": 832}});
        assert!(strip_named_fields(&mut body, &["foo".into(), "bar".into()]));
        assert!(body.get("foo").is_none());
        assert!(body["parameters"].get("bar").is_none());
        assert_eq!(body["parameters"]["width"], json!(832));
        assert!(!strip_named_fields(&mut body, &["missing".into()]));
    }
}
