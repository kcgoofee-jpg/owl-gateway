//! 连接测试请求的识别：SillyTavern 之类的「测试连接」会伪装成真实生图，按步数 / 面积 / 提示词长度认。

use super::*;

// ----------------------------------------------------------------------------- 连接测试请求

/// 「连接测试」请求的判定阈值。依据（线上日志 7 天、12,650 张成功出图）：流行的 SillyTavern 预设把「测试连接」
/// 当成真实生图发（提示词 `simple circle, white background`、512x512、4 步，还吃掉过 V5 额度）；
/// 另有客户端发提示词 `test`、64x64、1 步。7 天里没有任何正常请求同时满足「步数 ≤ 5 且面积 ≤ 512x512」，
/// 正常提示词平均约 1000 字符。三个阈值必须同时满足（再加「普通生图、单张、无角色」）才算测试。
pub const TEST_REQUEST_MAX_STEPS: i64 = 5;
/// 见 [`TEST_REQUEST_MAX_STEPS`]：宽 × 高不超过 512 × 512。
pub const TEST_REQUEST_MAX_PIXELS: i64 = 512 * 512;
/// 见 [`TEST_REQUEST_MAX_STEPS`]：提示词（去首尾空白后）不超过 40 个字符。
pub const TEST_REQUEST_MAX_PROMPT_CHARS: usize = 40;

/// 参数里出现任何一个（值为真）就说明不是单纯的文生图：图生图 / 局部重绘 / ControlNet / 参考图。
const NON_PLAIN_PARAMS: [&str; 6] = [
    "image",
    "mask",
    "controlnet_condition",
    "controlnet_model",
    "controlnet_strength",
    "reference_image",
];

/// 这是不是客户端的「连接测试」请求（见 [`TEST_REQUEST_MAX_STEPS`] 的依据）。要在
/// [`normalize_image_request`] 和提示词清理之后调用（尺寸 / 步数已校验为整数，提示词已按流水线规范化）。
///
/// 全部满足才算：动作是普通 generate（没有图生图 / 局部重绘 / 蒙版 / Vibe / 参考图 / ControlNet）、
/// 步数 ≤ 5、宽 × 高 ≤ 512 × 512、一次 ≤ 1 张、没有角色提示词、提示词 ≤ 40 字符
/// （V4 / V4.5 结构化提示词取 `base_caption`，否则取 `input`）。
/// 缺步数或尺寸（上游按默认值出大图）不算。
pub fn is_connection_test(body: &Value) -> bool {
    let Some(Value::Object(p)) = values::get(body, "parameters") else {
        return false;
    };
    let action = primitives::strip(&str_or_empty(values::get(body, "action"))).to_lowercase();
    if !action.is_empty() && action != "generate" {
        return false;
    }
    if NON_PLAIN_PARAMS
        .iter()
        .chain(REFERENCE_FIELDS.iter())
        .any(|k| enabled_opt(p.get(*k)))
        || ["image", "mask"]
            .iter()
            .any(|k| enabled_opt(values::get(body, k)))
        // 官网第二次用同一张 Vibe 时只发 cache_secret_key（数据在网关缓存里）——有材料就不是测试
        || enabled_opt(p.get("cache_secret_key"))
    {
        return false;
    }
    let int = |k: &str| p.get(k).and_then(Value::as_i64);
    let (Some(steps), Some(width), Some(height)) = (int("steps"), int("width"), int("height"))
    else {
        return false;
    };
    let n = int("n_samples").filter(|n| *n != 0).unwrap_or(1);
    if steps > TEST_REQUEST_MAX_STEPS
        || width.saturating_mul(height) > TEST_REQUEST_MAX_PIXELS
        || n > 1
    {
        return false;
    }
    if matches!(p.get("characterPrompts"), Some(Value::Array(a)) if !a.is_empty()) {
        return false;
    }
    let v4 = match p.get("v4_prompt") {
        Some(Value::Object(m)) => match m.get("caption") {
            Some(Value::Object(cap)) => Some(cap),
            _ => None,
        },
        _ => None,
    };
    if let Some(cap) = v4
        && matches!(cap.get("char_captions"), Some(Value::Array(a)) if !a.is_empty())
    {
        return false;
    }
    let prompt = match v4.and_then(|c| c.get("base_caption")) {
        Some(Value::String(s)) => s.as_str(),
        _ => match values::get(body, "input") {
            Some(Value::String(s)) => s.as_str(),
            _ => "",
        },
    };
    primitives::len_chars(primitives::strip(prompt)) <= TEST_REQUEST_MAX_PROMPT_CHARS
}

/// 「四无」裸空生图：正文、负面提示词都是 0 字，也没有角色提示词和任何 Vibe / 参考图 / 图生图材料。
/// 上游对 prompt=0 字直接 500（10-30 线上两次实锤），这种请求必然失败，在网关挡下来省一次排队
/// 和上游调用；带任一材料的空提示词是正当玩法（风格迁移 / 角色传承），不算裸空，放行。
/// 要在 `normalize_image_request` 之后调用：V4 / V4.5 / V5 的结构化提示词已填好。
pub fn is_blank_generation(body: &Value) -> bool {
    let Some(Value::Object(p)) = values::get(body, "parameters") else {
        return false;
    };
    if NON_PLAIN_PARAMS
        .iter()
        .chain(REFERENCE_FIELDS.iter())
        .any(|k| enabled_opt(p.get(*k)))
        || ["image", "mask"]
            .iter()
            .any(|k| enabled_opt(values::get(body, k)))
        // 官网第二次用同一张 Vibe 时只发 cache_secret_key，数据在网关进程内缓存里还没展开
        || enabled_opt(p.get("cache_secret_key"))
    {
        return false;
    }
    if matches!(p.get("characterPrompts"), Some(Value::Array(a)) if !a.is_empty()) {
        return false;
    }
    let has_char_captions = |key: &str| {
        matches!(
            p.get(key).and_then(|v| v.get("caption")).and_then(|c| c.get("char_captions")),
            Some(Value::Array(a)) if !a.is_empty()
        )
    };
    if has_char_captions("v4_prompt") || has_char_captions("v4_uc") {
        return false;
    }
    let cap = |key: &str| -> &str {
        p.get(key)
            .and_then(|v| v.get("caption"))
            .and_then(|c| c.get("base_caption"))
            .and_then(Value::as_str)
            .unwrap_or("")
    };
    // normalize 之后 V4 / V4.5 / V5 一定有 v4_prompt / v4_uc；input / negative_prompt 兜底防旧客户端
    let prompt = cap("v4_prompt");
    let uc = cap("v4_uc");
    let legacy_prompt = values::get(body, "input")
        .and_then(Value::as_str)
        .unwrap_or("");
    let legacy_uc = p
        .get("negative_prompt")
        .and_then(Value::as_str)
        .unwrap_or("");
    primitives::strip(prompt).is_empty()
        && primitives::strip(uc).is_empty()
        && primitives::strip(legacy_prompt).is_empty()
        && primitives::strip(legacy_uc).is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 归一化之后 V4.5 请求该有的形状（测试用）。
    fn v45_body(prompt: &str, uc: &str) -> Value {
        serde_json::json!({
            "model": "nai-diffusion-4-5-full",
            "input": prompt,
            "parameters": {
                "negative_prompt": uc,
                "width": 832, "height": 1216, "steps": 23,
                "v4_prompt": {"caption": {"base_caption": prompt, "char_captions": []}},
                "v4_uc": {"caption": {"base_caption": uc, "char_captions": []}}
            }
        })
    }

    #[test]
    fn blank_generation_rejects_only_the_four_nothing_case() {
        // 四无：正文/负面 0 字，无角色、无 Vibe —— 拦
        assert!(is_blank_generation(&v45_body("", "")));
        // 带正文 / 带负面 / 带角色 / 带 Vibe —— 都是正当玩法，放行
        assert!(!is_blank_generation(&v45_body("1girl", "")));
        assert!(!is_blank_generation(&v45_body("", "lowres")));
        let mut with_char = v45_body("", "");
        with_char["parameters"]["characterPrompts"] = json!([{"prompt": "owl girl"}]);
        assert!(!is_blank_generation(&with_char));
        let mut with_vibe = v45_body("", "");
        with_vibe["parameters"]["reference_image_multiple"] = json!(["cached"]);
        assert!(!is_blank_generation(&with_vibe));
        // 只发 cache_secret_key 的请求：材料在网关缓存里，不能当裸空拦
        let mut cached_vibe = v45_body("", "");
        cached_vibe["parameters"]["cache_secret_key"] = json!("abc");
        assert!(!is_blank_generation(&cached_vibe));
    }

    #[test]
    fn v5_structured_prompts_include_base_and_character_captions() {
        let payload = json!({
            "model": "nai-diffusion-5-curated",
            "input": "fur dataset, red owl",
            "parameters": {
                "negative_prompt": "lowres",
                "use_coords": true,
                "characterPrompts": [{
                    "prompt": "owl girl",
                    "uc": "bad hands",
                    "center": {"x": 0.25, "y": 0.75}
                }]
            }
        });
        let (filled, _) = fill_v4_prompt(&payload);
        assert_eq!(
            filled["parameters"]["v4_prompt"],
            json!({
                "caption": {
                    "base_caption": "fur dataset, red owl",
                    "char_captions": [{
                        "char_caption": "owl girl",
                        "centers": [{"x": 0.25, "y": 0.75}]
                    }]
                },
                "use_coords": true,
                "use_order": true
            })
        );
        assert_eq!(
            filled["parameters"]["v4_negative_prompt"],
            json!({
                "caption": {
                    "base_caption": "lowres",
                    "char_captions": [{
                        "char_caption": "bad hands",
                        "centers": [{"x": 0.25, "y": 0.75}]
                    }]
                },
                "legacy_uc": false
            })
        );
    }

    #[test]
    fn v5_structured_prompts_are_added_even_without_characters() {
        let payload = json!({
            "model": "nai-diffusion-5-full",
            "input": "simple circle",
            "parameters": {"negative_prompt": "lowres"}
        });
        let (filled, _) = fill_v4_prompt(&payload);
        assert_eq!(
            filled["parameters"]["v4_prompt"],
            json!({
                "caption": {"base_caption": "simple circle", "char_captions": []},
                "use_coords": false,
                "use_order": true
            })
        );
        assert_eq!(
            filled["parameters"]["v4_negative_prompt"]["caption"]["char_captions"],
            json!([])
        );
    }

    #[test]
    fn character_limits_follow_model_family() {
        let v5_chars: Vec<Value> = (0..MAX_V5_CHARACTERS)
            .map(|i| json!({"prompt": format!("character {i}")}))
            .collect();
        let v5 =
            json!({"model":"nai-diffusion-5-curated","parameters":{"characterPrompts":v5_chars}});
        assert_eq!(upstream_parameter_problem(&v5), None);

        let too_many_v5: Vec<Value> = (0..=MAX_V5_CHARACTERS)
            .map(|i| json!({"prompt": format!("character {i}")}))
            .collect();
        let v5 =
            json!({"model":"nai-diffusion-5-full","parameters":{"characterPrompts":too_many_v5}});
        assert!(
            upstream_parameter_problem(&v5)
                .unwrap()
                .contains("最多 22 个")
        );

        let v4_chars: Vec<Value> = (0..=MAX_CHARACTERS)
            .map(|i| json!({"prompt": format!("character {i}")}))
            .collect();
        let v4 =
            json!({"model":"nai-diffusion-4-5-full","parameters":{"characterPrompts":v4_chars}});
        assert!(
            upstream_parameter_problem(&v4)
                .unwrap()
                .contains("最多 6 个")
        );
    }

    #[test]
    fn legacy_reference_fields_are_named_in_the_error() {
        let req = |field: &str| json!({"model": "nai-diffusion-4-5-full", "parameters": {field: ["iVBORw0KGgo="]}});
        for (field, fix) in [
            (
                "director_reference_images",
                "director_reference_images_cached",
            ),
            ("characterReferences", "director_reference_images_cached"),
            ("reference_image", "reference_image_multiple"),
        ] {
            let msg = validate_image_references(&req(field)).expect("应拒绝");
            assert!(msg.contains(&format!(" {field}，")), "{msg}");
            assert!(msg.contains(fix) && msg.contains("未扣费"), "{msg}");
        }
        // 空数组不算用了旧写法
        assert_eq!(
            validate_image_references(
                &json!({"model": "nai-diffusion-4-5-full", "parameters": {"director_reference_images": []}})
            ),
            None
        );
    }

    // 8-byte PNG signature + IHDR start, enough for the magic check
    const PNG_B64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAAB";
    const JPEG_B64: &str = "/9j/4AAQSkZJRgABAQAAAQABAAD";

    fn precise_req(refs: Value) -> Value {
        let n = refs.as_array().map_or(0, Vec::len);
        let desc = json!({"caption": {"base_caption": "character&style", "char_captions": []}, "legacy_uc": false});
        json!({"model": "nai-diffusion-4-5-full", "parameters": {
            "director_reference_images": refs,
            "director_reference_descriptions": vec![desc; n],
            "director_reference_information_extracted": vec![1; n],
            "director_reference_strength_values": vec![1; n],
            "director_reference_secondary_strength_values": vec![0; n],
        }})
    }

    #[test]
    fn raw_precise_references_become_cached() {
        let mut body = precise_req(json!([PNG_B64, format!("data:image/png;base64,{PNG_B64}")]));
        convert_legacy_references(&mut body).expect("PNG is accepted");
        let p = &body["parameters"];
        assert!(p.get("director_reference_images").is_none());
        let cached = p["director_reference_images_cached"].as_array().unwrap();
        assert_eq!(cached.len(), 2);
        let key = cached[0]["cache_secret_key"].as_str().unwrap();
        assert!(is_hex64(key), "{key}");
        assert_eq!(cached[1]["cache_secret_key"], cached[0]["cache_secret_key"]); // same bytes, same key
        assert_eq!(cached[1]["data"], json!(PNG_B64)); // data URL prefix dropped
        assert_eq!(p["director_reference_strength_values"], json!([1, 1]));
        assert_eq!(validate_image_references(&body), None);
    }

    #[test]
    fn raw_precise_references_that_cannot_be_accepted() {
        let msg = convert_legacy_references(&mut precise_req(json!([JPEG_B64]))).unwrap_err();
        assert!(msg.contains("PNG") && msg.contains("未扣费"), "{msg}");
        let mut both = precise_req(json!([PNG_B64]));
        both["parameters"]["director_reference_images_cached"] =
            json!([{"cache_secret_key": "a".repeat(64), "data": PNG_B64}]);
        let msg = convert_legacy_references(&mut both).unwrap_err();
        assert!(msg.contains("重复提交"), "{msg}");
        let mut empty = precise_req(json!([]));
        convert_legacy_references(&mut empty).unwrap();
        assert!(
            empty["parameters"]
                .get("director_reference_images")
                .is_none()
        );
        // characterReferences stays rejected
        let mut cr = json!({"model": "nai-diffusion-4-5-full", "parameters": {"characterReferences": [PNG_B64]}});
        convert_legacy_references(&mut cr).unwrap();
        assert!(
            validate_image_references(&cr)
                .unwrap()
                .contains("characterReferences")
        );
    }

    #[test]
    fn uc_fills_a_missing_negative_prompt_only() {
        let mut b = json!({"model": "m", "parameters": {"uc": "lowres, bad hands"}});
        convert_legacy_references(&mut b).unwrap();
        assert_eq!(
            b["parameters"]["negative_prompt"],
            json!("lowres, bad hands")
        );
        let mut both = json!({"model": "m", "parameters": {"uc": "a", "negative_prompt": "b"}});
        convert_legacy_references(&mut both).unwrap();
        assert_eq!(both["parameters"]["negative_prompt"], json!("b"));
        let mut blank = json!({"model": "m", "parameters": {"uc": "  "}});
        convert_legacy_references(&mut blank).unwrap();
        assert!(blank["parameters"].get("negative_prompt").is_none());
    }

    fn probe(prompt: &str, steps: i64, w: i64, h: i64) -> Value {
        json!({"input": prompt, "model": "nai-diffusion-4-5-full", "action": "generate",
               "parameters": {"width": w, "height": h, "steps": steps, "n_samples": 1}})
    }

    #[test]
    fn connection_test_known_signatures() {
        // SillyTavern 预设
        let st = probe("simple circle, white background", 4, 512, 512);
        assert!(is_connection_test(&st));
        // 另一个客户端
        assert!(is_connection_test(&probe("test", 1, 64, 64)));
        // 同样的提示词，正常参数不算
        let normal = probe("simple circle, white background", 28, 832, 1216);
        assert!(!is_connection_test(&normal));
    }

    #[test]
    fn connection_test_boundaries() {
        let t = is_connection_test;
        assert!(t(&probe("a", 5, 512, 512)));
        assert!(!t(&probe("a", 6, 512, 512)));
        assert!(!t(&probe("a", 5, 513, 512)));
        assert!(t(&probe("a", 5, 256, 1024))); // 面积相同，形状不限
        assert!(t(&probe(&"x".repeat(40), 5, 512, 512)));
        assert!(!t(&probe(&"x".repeat(41), 5, 512, 512)));
        // 首尾空白不算；按字符数而不是字节数
        assert!(t(&probe(&format!("  {}  ", "x".repeat(40)), 5, 512, 512)));
        assert!(t(&probe(&"猫".repeat(40), 5, 512, 512)));
        assert!(!t(&probe(&"猫".repeat(41), 5, 512, 512)));
    }

    #[test]
    fn connection_test_requires_plain_single_generate() {
        let t = is_connection_test;
        let mut p = probe("test", 1, 64, 64);
        p["action"] = json!("img2img");
        assert!(!t(&p));
        p["action"] = json!("infill");
        assert!(!t(&p));
        p["action"] = json!("GENERATE");
        assert!(t(&p));
        let mut p = probe("test", 1, 64, 64);
        p["parameters"]["image"] = json!("QUJD");
        assert!(!t(&p));
        let mut p = probe("test", 1, 64, 64);
        p["parameters"]["mask"] = json!("QUJD");
        assert!(!t(&p));
        let mut p = probe("test", 1, 64, 64);
        p["parameters"]["reference_image_multiple"] = json!(["QUJD"]);
        assert!(!t(&p));
        let mut p = probe("test", 1, 64, 64);
        p["parameters"]["director_reference_images_cached"] = json!([{"cache_secret_key": "k"}]);
        assert!(!t(&p));
        let mut p = probe("test", 1, 64, 64);
        p["parameters"]["controlnet_condition"] = json!("QUJD");
        assert!(!t(&p));
        // 空的参考图列表不算
        let mut p = probe("test", 1, 64, 64);
        p["parameters"]["reference_image_multiple"] = json!([]);
        assert!(t(&p));
        let mut p = probe("test", 1, 64, 64);
        p["parameters"]["n_samples"] = json!(2);
        assert!(!t(&p));
        let mut p = probe("test", 1, 64, 64);
        p["parameters"]["characterPrompts"] = json!([{"prompt": "girl"}]);
        assert!(!t(&p));
        // 缺步数 / 尺寸：上游按默认值出大图，不算测试
        let mut p = probe("test", 1, 64, 64);
        p["parameters"].as_object_mut().unwrap().remove("steps");
        assert!(!t(&p));
        let mut p = probe("test", 1, 64, 64);
        p["parameters"].as_object_mut().unwrap().remove("width");
        assert!(!t(&p));
        assert!(!t(&json!({"input": "test", "model": "x"})));
    }

    #[test]
    fn connection_test_uses_v4_base_caption() {
        let t = is_connection_test;
        let mut p = probe("", 4, 512, 512);
        p["parameters"]["v4_prompt"] =
            json!({"caption": {"base_caption": "simple circle", "char_captions": []}});
        assert!(t(&p));
        p["parameters"]["v4_prompt"]["caption"]["base_caption"] = json!("x".repeat(41));
        assert!(!t(&p));
        p["parameters"]["v4_prompt"]["caption"]["base_caption"] = json!("ok");
        p["parameters"]["v4_prompt"]["caption"]["char_captions"] =
            json!([{"char_caption": "girl"}]);
        assert!(!t(&p));
    }

    #[test]
    fn strict_base64_matches_binascii_rules() {
        assert!(strict_b64("QQ==", 12).is_some());
        assert!(strict_b64("QUI=", 12).is_some());
        assert!(strict_b64("QUJD", 12).is_some());
        assert!(strict_b64("", 12).is_some());
        for bad in [
            "QQ=", "QQ", "Q", "A=", "=QQ=", "QQ==QQ==", "QUJD====", "QQ ==", "QUJD\n", "QUI==",
            "QQ=Q", "-_",
        ] {
            assert!(strict_b64(bad, 12).is_none(), "{bad}");
        }
        let (head, total) = strict_b64("QUJDREVGR0hJSktMTU4=", 4).unwrap();
        assert_eq!(head, b"ABCD");
        assert_eq!(total, 14);
    }

    #[test]
    fn huge_base64_is_rejected_without_decoding() {
        let big = Value::String("A".repeat(25 * 1024 * 1024 + 4));
        assert!(!base64_data(Some(&big), false, false));
        let edge = Value::String("A".repeat(25 * 1024 * 1024));
        assert!(base64_data(Some(&edge), false, false));
    }

    #[test]
    fn prompt_cleanup_samples() {
        assert_eq!(
            clean_prompt_syntax("(red hair:1.2), (blue eyes:0.8)"),
            "1.2::red hair::, 0.8::blue eyes::"
        );
        assert_eq!(clean_prompt_syntax("a,, b ,::, c"), "a, b, c");
        let mut body = json!({"input": "bm94199::, 1.2::tag::"});
        assert_eq!(fix_name_digit_weights(&mut body), 1);
        assert_eq!(body["input"], "bm94199 ::, 1.2::tag::");
    }

    #[test]
    fn gen_key_format() {
        let k = gen_key("nai", &[0; 24]);
        assert_eq!(k, format!("nai-{}", "A".repeat(32)));
    }

    #[test]
    fn k_dpm_2_rejected_on_v5_only() {
        let body = |model: &str| json!({"model": model, "parameters": {"sampler": "k_dpm_2", "width": 832, "height": 1216, "steps": 28}});
        let v5 = upstream_parameter_problem(&body("nai-diffusion-5-full")).expect("V5 rejected");
        assert!(v5.contains("k_dpm_2 不支持 V5"), "{v5}");
        assert_eq!(
            upstream_parameter_problem(&body("nai-diffusion-4-5-full")),
            None
        );
    }
}
