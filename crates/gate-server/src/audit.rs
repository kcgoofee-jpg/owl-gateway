//! Prompt metadata captured independently from image delivery.
use serde_json::{Map, Value};
pub const RETENTION_DAYS: i64 = gate_core::PROMPT_RETENTION_DAYS;
const PARAM_KEYS: [&str; 22] = [
    "seed",
    "sampler",
    "steps",
    "scale",
    "width",
    "height",
    "noise_schedule",
    "cfg_rescale",
    "sm",
    "sm_dyn",
    "ucPreset",
    "qualityToggle",
    "n_samples",
    // 以下影响复现（Variety+、采样行为、角色坐标开关）
    "skip_cfg_above_sigma",
    "uncond_scale",
    "legacy",
    "legacy_v3_extend",
    "legacy_uc",
    "prefer_brownian",
    "deliberate_euler_ancestral_bug",
    "params_version",
    "use_coords",
];

fn text(value: Option<&Value>, max: usize) -> String {
    value
        .and_then(Value::as_str)
        .unwrap_or_default()
        .chars()
        .take(max)
        .collect()
}
fn captions(node: &Value) -> (String, Vec<Value>, Vec<Value>) {
    let caption = &node["caption"];
    let mut roles = Vec::new();
    let mut centers = Vec::new();
    if let Some(items) = caption["char_captions"].as_array() {
        for item in items.iter().take(12) {
            let value = text(item.get("char_caption"), 4000);
            if !value.is_empty() {
                roles.push(Value::String(value));
                centers.push(
                    item.get("centers")
                        .filter(|v| v.is_array())
                        .cloned()
                        .unwrap_or_else(|| serde_json::json!([])),
                );
            }
        }
    }
    (text(caption.get("base_caption"), 4000), roles, centers)
}
pub fn capture_prompts(body: &Value) -> (String, String, String) {
    let params = &body["parameters"];
    let (pos_base, pos_roles, pos_centers) = captions(&params["v4_prompt"]);
    let (neg_base, neg_roles, neg_centers) = captions(&params["v4_negative_prompt"]);
    let positive = text(body.get("input"), 4000);
    let negative = text(params.get("negative_prompt"), 2000);
    let mut kept = Map::new();
    for key in PARAM_KEYS {
        if let Some(v) = params.get(key).filter(|v| !v.is_null()) {
            kept.insert(key.to_owned(), v.clone());
        }
    }
    let extra = serde_json::json!({"params": kept, "char_prompts": pos_roles, "char_negatives": neg_roles, "char_centers": pos_centers, "char_negative_centers": neg_centers});
    (
        if positive.is_empty() {
            pos_base
        } else {
            positive
        },
        if negative.is_empty() {
            neg_base
        } else {
            negative
        },
        extra.to_string(),
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_roles_and_valid_parameter_json() {
        let body = serde_json::json!({"input":"x", "parameters":{"seed":42,"v4_prompt":{"caption":{"char_captions":[{"char_caption":"role","centers":[{"x":0.5,"y":0.5}]}]}}}});
        let (positive, _, extra) = capture_prompts(&body);
        assert_eq!(positive, "x");
        let extra: Value = serde_json::from_str(&extra).unwrap();
        assert_eq!(extra["params"]["seed"], 42);
        assert_eq!(extra["char_prompts"][0], "role");
        assert_eq!(extra["char_centers"][0][0]["x"], 0.5);
    }
    #[test]
    fn malformed_optional_fields_do_not_drop_record() {
        let (p, n, extra) = capture_prompts(
            &serde_json::json!({"input":false,"parameters":{"v4_prompt":{"caption":5}}}),
        );
        assert!(p.is_empty() && n.is_empty());
        assert!(serde_json::from_str::<Value>(&extra).is_ok());
    }
}
