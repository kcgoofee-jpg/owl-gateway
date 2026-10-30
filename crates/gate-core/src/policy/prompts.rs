//! 提示词处理：收集、权重清洗、翻译与清洗、V4 结构化提示词、V5 Medium 预设。

use super::*;

// ----------------------------------------------------------------------------- 提示词

/// 按固定顺序列出所有提示词位置：`(路径, 值)`，值可能不是字符串（由调用方判断）。
pub fn prompt_texts(body: &Value) -> Vec<(String, Option<&Value>)> {
    let mut out: Vec<(String, Option<&Value>)> =
        vec![("input".to_string(), values::get(body, "input"))];
    let Some(Value::Object(p)) = values::get(body, "parameters") else {
        return out;
    };
    for key in ["prompt", "negative_prompt"] {
        out.push((format!("parameters.{key}"), p.get(key)));
    }
    if let Some(Value::Array(items)) = p.get("characterPrompts") {
        for (i, item) in items.iter().enumerate() {
            if let Value::Object(m) = item {
                out.push((
                    format!("parameters.characterPrompts[{i}].prompt"),
                    m.get("prompt"),
                ));
                out.push((format!("parameters.characterPrompts[{i}].uc"), m.get("uc")));
            }
        }
    }
    for key in ["v4_prompt", "v4_negative_prompt"] {
        let caption = match p.get(key) {
            Some(Value::Object(m)) => m.get("caption"),
            _ => None,
        };
        let Some(Value::Object(cap)) = caption else {
            continue;
        };
        out.push((format!("{key}.base_caption"), cap.get("base_caption")));
        if let Some(Value::Array(chars)) = cap.get("char_captions") {
            for (i, item) in chars.iter().enumerate() {
                if let Value::Object(m) = item {
                    out.push((format!("{key}.char_captions[{i}]"), m.get("char_caption")));
                }
            }
        }
    }
    out
}

/// 对请求里每个字符串形态的提示词字段原地改写（顺序固定，快照测试按它对拍）。
pub(crate) fn rewrite_prompt_strings(body: &mut Value, f: &mut dyn FnMut(&str) -> String) {
    fn fix(slot: Option<&mut Value>, f: &mut dyn FnMut(&str) -> String) {
        if let Some(Value::String(s)) = slot {
            *s = f(s);
        }
    }
    fix(body.get_mut("input"), f);
    let Some(Value::Object(p)) = body.get_mut("parameters") else {
        return;
    };
    for key in ["prompt", "negative_prompt"] {
        fix(p.get_mut(key), f);
    }
    if let Some(Value::Array(items)) = p.get_mut("characterPrompts") {
        for item in items {
            if let Value::Object(m) = item {
                for key in ["prompt", "uc"] {
                    fix(m.get_mut(key), f);
                }
            }
        }
    }
    for key in ["v4_prompt", "v4_negative_prompt"] {
        let Some(Value::Object(v4)) = p.get_mut(key) else {
            continue;
        };
        let Some(Value::Object(cap)) = v4.get_mut("caption") else {
            continue;
        };
        fix(cap.get_mut("base_caption"), f);
        if let Some(Value::Array(chars)) = cap.get_mut("char_captions") {
            for item in chars {
                if let Value::Object(m) = item {
                    fix(m.get_mut("char_caption"), f);
                }
            }
        }
    }
}

pub(crate) struct Regexes {
    /// `(?<=[A-Za-z_])(\d+(?:\.\d+)?)::`
    name_digits: FancyRegex,
    /// `\(([^:\(\)]+):(\d+(?:\.\d+)?)\)`
    webui: Regex,
    /// `(?:^|(?<=,))\s*::\s*(?:$|(?=,))`
    isolated_colons: FancyRegex,
    /// `,\s*,+`
    consecutive_commas: Regex,
    /// `\s*,\s*`
    around_comma: Regex,
    /// `(?<![\d.])([-+]?\d+(?:\.\d+)?)::`
    weight: FancyRegex,
}

pub(crate) fn fancy(pattern: &str) -> FancyRegex {
    // 回溯上限关掉：这些模式在输入长度上是线性的，关掉上限避免长输入误报错
    fancy_regex::RegexBuilder::new(pattern)
        .backtrack_limit(usize::MAX)
        .build()
        .expect("内置正则必须合法")
}

/// `\d` / `\s` 展开成固定的字符类，避免 regex 自带 Unicode 表版本更新带来的差异。
pub(crate) fn regexes() -> &'static Regexes {
    static R: OnceLock<Regexes> = OnceLock::new();
    R.get_or_init(|| {
        let nd = nd_class();
        let d = format!("[{nd}]");
        let s = format!("[{}]", space_class());
        Regexes {
            name_digits: fancy(&format!(r"(?<=[A-Za-z_])({d}+(?:\.{d}+)?)::")),
            webui: Regex::new(&format!(r"\(([^:()]+):({d}+(?:\.{d}+)?)\)"))
                .expect("内置正则必须合法"),
            isolated_colons: fancy(&format!(r"(?:^|(?<=,)){s}*::{s}*(?:$|(?=,))")),
            consecutive_commas: Regex::new(&format!(r",{s}*,+")).expect("内置正则必须合法"),
            around_comma: Regex::new(&format!(r"{s}*,{s}*")).expect("内置正则必须合法"),
            weight: fancy(&format!(r"(?<![{nd}.])([-+]?{d}+(?:\.{d}+)?)::")),
        }
    })
}

/// `re.subn`：返回替换后的文本和替换次数；`rep` 往输出里写替换内容。
pub(crate) fn fancy_sub(
    re: &FancyRegex,
    text: &str,
    mut rep: impl FnMut(&fancy_regex::Captures<'_, str>, &mut String),
) -> (String, usize) {
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    let mut n = 0;
    for caps in re.captures_iter(text) {
        let caps = caps.expect("回溯上限已关闭，不会有运行期错误");
        let m = caps.get(0).expect("第 0 组总存在");
        out.push_str(&text[last..m.start()]);
        rep(&caps, &mut out);
        last = m.end();
        n += 1;
    }
    out.push_str(&text[last..]);
    (out, n)
}

/// 名字结尾的数字紧挨着 `::`（如画师 `bm94199::`）时，在 `::` 前补一个空格，原地改写，返回改了几处。
///
/// 本意是用 `::` 结束一段加权，但上游会把 94199 当成权重，结果 NaN。
/// 只改「数字前面紧挨着字母或下划线」的情况，独立的 `1.2::` / `-3::` 这类正常权重不动。
pub fn fix_name_digit_weights(body: &mut Value) -> usize {
    let re = &regexes().name_digits;
    let mut n = 0;
    rewrite_prompt_strings(body, &mut |text| {
        if !text.contains("::") {
            return text.to_string();
        }
        let (new, k) = fancy_sub(re, text, |caps, out| {
            out.push_str(&caps[1]);
            out.push_str(" ::");
        });
        n += k;
        new
    });
    n
}

/// 清理单条提示词文本：
/// 1. 将 WebUI 权重语法 `(tag:1.2)` 转译为 NAI 原生 `1.2::tag::`；
/// 2. 清理因客户端切词手抖留下的孤立破损 `::`；
/// 3. 合并多余连续逗号与修剪首尾。
pub fn clean_prompt_syntax(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    let r = regexes();
    let t1 = r
        .webui
        .replace_all(text, |c: &regex::Captures<'_>| {
            format!("{}::{}::", &c[2], &c[1])
        })
        .into_owned();
    let (t2, _) = fancy_sub(&r.isolated_colons, &t1, |_, _| {});
    let t3 = r.consecutive_commas.replace_all(&t2, ",").into_owned();
    let t4 = r.around_comma.replace_all(&t3, ", ").into_owned();
    t4.trim_matches(|c| c == ' ' || c == ',').to_string()
}

/// 扫描请求中所有的提示词字段，执行语法自愈与转译，返回修改字段数。
pub fn sanitize_and_translate_prompts(body: &mut Value) -> usize {
    let mut n = 0;
    rewrite_prompt_strings(body, &mut |text| {
        if text.is_empty() {
            return String::new();
        }
        let cleaned = clean_prompt_syntax(text);
        if cleaned != text {
            n += 1;
        }
        cleaned
    });
    n
}

/// 在排队之前拦下上游一定会拒绝的请求，避免白占全站唯一的出图队列。
pub fn upstream_parameter_problem(body: &Value) -> Option<String> {
    let weight = &regexes().weight;
    for (path, text) in prompt_texts(body) {
        let Some(Value::String(text)) = text else {
            continue;
        };
        let size = text.len(); // UTF-8 字节数
        if size >= MAX_PROMPT_BYTES {
            return Some(format!(
                "提示词过长：{size} 字节，上限 {MAX_PROMPT_BYTES} 字节，每个汉字计 3 字节，未扣费。请缩短提示词。开发者参考：超长字段为 {path}"
            ));
        }
        // 不按括号层数拦截（NAI 花括号每层只 ×1.05）。NaN 的真实原因是超大数值权重。
        // 回看断言 `(?<![\d.])`：长串数字不再 O(n²)。
        for caps in weight.captures_iter(text) {
            let caps = caps.expect("回溯上限已关闭，不会有运行期错误");
            let g = caps.get(1).expect("第 1 组总参与匹配").as_str();
            let Some(val) = primitives::parse_float(g) else {
                continue;
            };
            if val.abs() > 100.0 {
                return Some(format!(
                    "提示词中有异常权重 {g}::，通常是画师名中的数字紧挨着 ::，未扣费。请在 :: 前加逗号或空格。"
                ));
            }
        }
    }
    let p = match values::get(body, "parameters") {
        Some(Value::Object(m)) => m,
        _ => empty_obj(),
    };
    let generate = match values::get(body, "action") {
        None => true,
        Some(Value::String(a)) => a == "generate",
        Some(_) => false,
    };
    // 10-31：V4 / V4.5 同时打开 SMEA 和 sm_dyn 时，官方直接回 500（线上 7 天 60 次上游 5xx 里
    // 有相当一部分是这个组合）。网关先挡下来，给一句能照做的提示，不要让用户以为是我们挂了。
    let flag = |k: &str| p.get(k).map(values::enabled).unwrap_or(false);
    if flag("sm") && flag("sm_dyn") {
        return Some(
            "SMEA 和 sm_dyn 不能同时打开（官方会直接报内部错误），未扣费。请关掉其中一个再试。"
                .to_string(),
        );
    }
    if generate {
        for key in ["width", "height"] {
            let Some(value) = p.get(key).and_then(int_like) else {
                continue;
            };
            let step = i128::from(DIMENSION_STEP);
            if value > 0 && value % step != 0 {
                let lower = std::cmp::max(step, value / step * step);
                // bool 也是 int：True 会走到这里并被打印成 "True"
                let shown = match p.get(key) {
                    Some(Value::Bool(b)) => primitives::bool_text(*b).to_string(),
                    _ => value.to_string(),
                };
                let side = if key == "width" { "宽度" } else { "高度" };
                return Some(format!(
                    "图片{side} {shown} 不是 64 的倍数，未扣费。请改为 {lower} 或 {}。",
                    lower + step
                ));
            }
        }
    }
    let model = str_of_get(body.as_object().unwrap_or(empty_obj()), "model");
    if model.starts_with("nai-diffusion-4") || model.starts_with("nai-diffusion-5") {
        // V4 / V4.5 最多 6 个角色，V5 最多 22 个。
        let mut counts = vec![match p.get("characterPrompts") {
            Some(Value::Array(a)) => a.len(),
            _ => 0,
        }];
        for key in ["v4_prompt", "v4_negative_prompt"] {
            let caption = match p.get(key) {
                Some(Value::Object(m)) => m.get("caption"),
                _ => None,
            };
            if let Some(Value::Object(cap)) = caption
                && let Some(Value::Array(a)) = cap.get("char_captions")
            {
                counts.push(a.len());
            }
        }
        let max = counts.into_iter().max().unwrap_or(0);
        let limit = if model.starts_with("nai-diffusion-5") {
            MAX_V5_CHARACTERS
        } else {
            MAX_CHARACTERS
        };
        if max > limit {
            let family = if model.starts_with("nai-diffusion-5") {
                "V5"
            } else {
                "V4/V4.5"
            };
            return Some(format!(
                "角色数量超限：本次 {max} 个，{family} 最多 {limit} 个，未扣费。请减少角色。"
            ));
        }
    }
    if let Some(sampler) = p.get("sampler")
        && !sampler.is_null()
    {
        let valid = match sampler {
            Value::String(s) => {
                let stripped = s.replace('_', "");
                s.is_ascii()
                    && primitives::len_chars(s) <= 64
                    && !stripped.is_empty()
                    && stripped.bytes().all(|b| b.is_ascii_alphanumeric())
            }
            _ => false,
        };
        if !valid {
            return Some(
                "采样器名称无效，未扣费。请改用官方标准采样器，如 k_euler_ancestral。".into(),
            );
        }
        let name = sampler.as_str().unwrap_or("");
        if in_models(&V4_V5_REJECTED_SAMPLERS, name)
            && (is_v5_model(&model) || model.starts_with("nai-diffusion-4"))
        {
            return Some(format!(
                "采样器 {name} 不支持 V4/V4.5/V5 模型，未扣费。请改用 k_euler_ancestral、k_dpmpp_2m 等。"
            ));
        }
        if in_models(&V5_REJECTED_SAMPLERS, name) && is_v5_model(&model) {
            return Some(format!(
                "采样器 {name} 不支持 V5 模型，未扣费。请改用 k_euler_ancestral、k_dpmpp_2m 等。"
            ));
        }
    }
    if let Some(schedule) = p.get("noise_schedule")
        && !schedule.is_null()
    {
        let valid =
            matches!(schedule, Value::String(s) if s.is_ascii() && primitives::len_chars(s) <= 32);
        if !valid {
            return Some("噪声调度 noise_schedule 设置无效，未扣费。请在客户端中恢复默认值。给客户端作者：noise_schedule 须为不超过 32 个字符的 ASCII 字符串".into());
        }
    }
    None
}

// ----------------------------------------------------------------------------- V4 结构化提示词

pub(crate) fn centers(c: &Obj) -> Value {
    let default = || float_value(0.5);
    let cen = match c.get("center") {
        Some(Value::Object(m)) => Some(m),
        _ => None,
    };
    let get = |k: &str| cen.and_then(|m| m.get(k)).cloned().unwrap_or_else(default);
    let mut pt = Obj::new();
    pt.insert("x".into(), get("x"));
    pt.insert("y".into(), get("y"));
    Value::Array(vec![Value::Object(pt)])
}

/// V4 / V4.5 / V5 使用 `v4_prompt` / `v4_negative_prompt` 角色结构。
/// 官方网页端用 input / negative_prompt / characterPrompts 拼出这两个结构，这里照做。
pub fn fill_v4_prompt(payload: &Value) -> (Value, Vec<String>) {
    let Some(src) = payload.as_object() else {
        return (payload.clone(), Vec::new());
    };
    let model = str_of_get(src, "model");
    let Some(Value::Object(p0)) = src.get("parameters") else {
        return (payload.clone(), Vec::new());
    };
    if !model.starts_with("nai-diffusion-4") && !model.starts_with("nai-diffusion-5") {
        return (payload.clone(), Vec::new());
    }
    let mut notes = Vec::new();
    let mut out = src.clone();
    let mut p = p0.clone();
    let chars: Vec<&Obj> = match p0.get("characterPrompts") {
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_object).collect(),
        _ => Vec::new(),
    };
    let obj = |entries: Vec<(&str, Value)>| -> Obj {
        entries
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect()
    };
    if !matches!(p.get("v4_prompt"), Some(Value::Object(_))) {
        let char_captions: Vec<Value> = chars
            .iter()
            .map(|c| {
                Value::Object(obj(vec![
                    ("char_caption", Value::String(str_or_empty(c.get("prompt")))),
                    ("centers", centers(c)),
                ]))
            })
            .collect();
        let caption = obj(vec![
            (
                "base_caption",
                Value::String(str_or_empty(src.get("input"))),
            ),
            ("char_captions", Value::Array(char_captions)),
        ]);
        p.insert(
            "v4_prompt".into(),
            Value::Object(obj(vec![
                ("caption", Value::Object(caption)),
                ("use_coords", Value::Bool(enabled_opt(p0.get("use_coords")))),
                ("use_order", Value::Bool(true)),
            ])),
        );
        notes.push("已按官方格式补上 v4_prompt".to_string());
    }
    if !matches!(p.get("v4_negative_prompt"), Some(Value::Object(_))) {
        let char_captions: Vec<Value> = chars
            .iter()
            .map(|c| {
                Value::Object(obj(vec![
                    ("char_caption", Value::String(str_or_empty(c.get("uc")))),
                    ("centers", centers(c)),
                ]))
            })
            .collect();
        let caption = obj(vec![
            (
                "base_caption",
                Value::String(str_or_empty(p0.get("negative_prompt"))),
            ),
            ("char_captions", Value::Array(char_captions)),
        ]);
        p.insert(
            "v4_negative_prompt".into(),
            Value::Object(obj(vec![
                ("caption", Value::Object(caption)),
                ("legacy_uc", Value::Bool(false)),
            ])),
        );
        notes.push("已按官方格式补上 v4_negative_prompt".to_string());
    }
    out.insert("parameters".into(), Value::Object(p));
    (Value::Object(out), notes)
}

// ----------------------------------------------------------------------------- V5 Medium

/// V5 Full（含局部重绘）对应的 Medium 模型；不是这两个模型（含已经是 Medium、Curated、别名）返回 None。
pub fn v5_medium_of(model: &str) -> Option<&'static str> {
    let m = primitives::strip(model).to_lowercase();
    V5_MEDIUM_OF
        .iter()
        .find(|(full, _)| *full == m)
        .map(|(_, medium)| *medium)
}

pub fn is_v5_medium(model: &str) -> bool {
    let m = primitives::strip(model).to_lowercase();
    V5_MEDIUM_OF.iter().any(|(_, medium)| *medium == m)
}

/// `{**c, key: ""}`：保留原键顺序，键已存在则原位覆盖，不存在则追加。
pub(crate) fn with_blank(c: &Value, key: &str) -> Value {
    match c {
        Value::Object(m) => {
            let mut m2 = m.clone();
            m2.insert(key.into(), Value::String(String::new()));
            Value::Object(m2)
        }
        other => other.clone(),
    }
}

/// 按官方前端对 Medium 的做法整理参数：步数 / 采样器 / 负面预设固定，自定义负面词清空，rescale 归零。
pub fn medium_normalize(payload: &Value) -> (Value, Vec<String>) {
    let mut out = payload.as_object().cloned().unwrap_or_default();
    let mut p = params_copy(&out);
    out.insert("parameters".into(), Value::Null);
    let mut notes = Vec::new();
    // `p.get("steps") != 14`：14.0 == 14，不会被改写
    if !num_eq_int(p.get("steps"), i128::from(V5_MEDIUM_STEPS)) {
        p.insert("steps".into(), Value::from(V5_MEDIUM_STEPS));
    }
    if p.get("sampler").and_then(Value::as_str) != Some(V5_MEDIUM_SAMPLER) {
        p.insert("sampler".into(), Value::String(V5_MEDIUM_SAMPLER.into()));
    }
    // `(x or 0) != 0`：任何真值都不等于 0
    if enabled_opt(p.get("cfg_rescale")) {
        p.insert("cfg_rescale".into(), Value::from(0));
        notes.push("Medium 不支持 rescale，已设为 0".into());
    }
    let user_uc = primitives::strip(&str_or_empty(p.get("negative_prompt"))).to_string();
    if !user_uc.is_empty() && user_uc != V5_MEDIUM_UC {
        notes.push(
            "Medium 不支持自定义负面词，已换成官方 Heavy 预设（可在正面提示词里用 -2::xx:: 排除）"
                .into(),
        );
    }
    p.insert("negative_prompt".into(), Value::String(V5_MEDIUM_UC.into()));
    p.insert("ucPreset".into(), Value::from(V5_MEDIUM_UC_PRESET));
    if let Some(Value::Object(neg0)) = p.get("v4_negative_prompt") {
        let mut neg = neg0.clone();
        let mut cap = match neg.get("caption") {
            Some(Value::Object(m)) => m.clone(),
            _ => Obj::new(),
        };
        cap.insert("base_caption".into(), Value::String(V5_MEDIUM_UC.into()));
        if let Some(Value::Array(cc)) = cap.get("char_captions") {
            let blanked: Vec<Value> = cc.iter().map(|c| with_blank(c, "char_caption")).collect();
            cap.insert("char_captions".into(), Value::Array(blanked));
        }
        neg.insert("caption".into(), Value::Object(cap));
        p.insert("v4_negative_prompt".into(), Value::Object(neg));
    }
    if let Some(Value::Array(cps)) = p.get("characterPrompts") {
        let blanked: Vec<Value> = cps.iter().map(|c| with_blank(c, "uc")).collect();
        p.insert("characterPrompts".into(), Value::Array(blanked));
    }
    out.insert("parameters".into(), Value::Object(p));
    (Value::Object(out), notes)
}
