//! 尺寸钳制的两条规则，分开钉住：
//!
//! - **V5**：上游只认三个 Normal 预设。落在某个预设框内的照原样发（文档口径是「之内」），超出框按长宽比吸附。
//! - **V4.5 及以下**：支持任意 64 的倍数，只按面积等比缩小，**不吸附 V5 预设**——吸附会换掉构图
//!   （1902×1080 变 1216×832、65536×64 变 1216×832），那不是缩小。
//!
//! 另外钉住 `fit_size` 的取整：超面积时一档一档缩长边，两边相等时同时缩，正方形不会被缩歪。

use serde_json::json;

fn clamp(model: &str, w: i64, h: i64) -> (i64, i64) {
    let body = json!({"model": model, "parameters": {"width": w, "height": h, "steps": 28}});
    let (out, _notes, err) =
        gate_core::policy::clamp_image_params(&body, 1024 * 1024, 28, true).unwrap();
    assert!(err.is_none(), "{model} {w}x{h} 被拒绝：{err:?}");
    let p = &out["parameters"];
    (p["width"].as_i64().unwrap(), p["height"].as_i64().unwrap())
}

const V45: &str = "nai-diffusion-4-5-full";
const V5: &str = "nai-diffusion-5-full";

#[test]
fn old_models_keep_their_aspect_ratio_and_are_never_snapped_to_v5_presets() {
    // 16:9 应该缩成 16:9，不是 V5 的 1216×832
    assert_eq!(clamp(V45, 1920, 1080), (1344, 768));
    // 3:2 缩成 3:2 附近（64 取整后 1.46）
    assert_eq!(clamp(V45, 1536, 1024), (1216, 832));
    // 落在面积上限内的自定义尺寸原样保留
    assert_eq!(clamp(V45, 768, 1280), (768, 1280));
    assert_eq!(clamp(V45, 1024, 1024), (1024, 1024));
    // 正方形超面积 → 还是正方形
    assert_eq!(clamp(V45, 1216, 1216), (1024, 1024));
}

#[test]
fn v5_snaps_only_when_the_request_leaves_the_preset_box() {
    // 三个预设原样通过
    assert_eq!(clamp(V5, 832, 1216), (832, 1216));
    assert_eq!(clamp(V5, 1216, 832), (1216, 832));
    assert_eq!(clamp(V5, 1024, 1024), (1024, 1024));
    // 落在框内（768 ≤ 832、512 ≤ 1216）不吸附
    assert_eq!(clamp(V5, 768, 512), (768, 512));
    // 超出框：按长宽比吸附
    assert_eq!(clamp(V5, 1920, 1080), (1216, 832));
    assert_eq!(clamp(V5, 768, 1280), (832, 1216));
    assert_eq!(clamp(V5, 1216, 1216), (1024, 1024));
}

#[test]
fn fit_size_keeps_wide_and_tall_requests_proportional() {
    let fit = |w, h, cap| gate_core::policy::fit_size(w, h, cap).unwrap();
    assert_eq!(fit(1920, 1080, 1024 * 1024), (1344, 768));
    assert_eq!(fit(1080, 1920, 1024 * 1024), (768, 1344));
    // 正方形不会被缩歪
    let (w, h) = fit(4096, 4096, 2 * 1024 * 1024);
    assert_eq!(w, h, "正方形缩完还得是正方形：{w}x{h}");
    // 本来就够小：原样返回（幂等）
    assert_eq!(fit(512, 512, 1024 * 1024), (512, 512));
}

/// 尺寸常量的相互关系：改任何一个都必须让这些成立，否则上层会算出上游不收的尺寸。
#[test]
fn size_constants_stay_consistent() {
    use gate_core::policy as p;

    // 三个 V5 预设必须落在 64 网格上（上游硬要求），且都不超过免费面积
    for (w, h) in p::V5_NORMAL_PRESETS {
        assert_eq!(w % p::DIMENSION_STEP, 0, "预设宽 {w} 不在 64 网格上");
        assert_eq!(h % p::DIMENSION_STEP, 0, "预设高 {h} 不在 64 网格上");
        let area = w * h;
        assert!(
            area <= p::FREE_MAX_PIXELS,
            "预设 {w}×{h} 面积 {area} 超过免费上限 {}",
            p::FREE_MAX_PIXELS
        );
    }

    // 「Normal」基准 = 长方形预设（832×1216）的面积，也是三个预设里最小的：比它还小的图才会被工具放大
    let areas: Vec<i64> = p::V5_NORMAL_PRESETS.iter().map(|(w, h)| w * h).collect();
    assert_eq!(
        p::NORMAL_PIXELS as i64,
        *areas.iter().min().unwrap(),
        "NORMAL_PIXELS 应等于最小的 V5 预设面积"
    );

    // 工具上限比免费上限宽，且是 64 网格上的尺寸（1536×2048）
    assert!(p::TOOL_MAX_PIXELS > p::FREE_MAX_PIXELS as u64);
    assert_eq!(p::TOOL_MAX_PIXELS, 1536 * 2048);
    // 任何一边都不能超过解码上限
    assert!((p::TOOL_MAX_PIXELS as f64).sqrt() < f64::from(p::MAX_SIDE));

    // fit_size 的输出永远在网格上、永远不超上限
    for cap in [p::FREE_MAX_PIXELS, p::FREE_MAX_PIXELS / 4] {
        for (w, h) in [
            (1920, 1080),
            (1080, 1920),
            (3000, 3000),
            (4096, 512),
            (512, 4096),
        ] {
            let (ow, oh) = p::fit_size(w, h, i128::from(cap)).unwrap();
            assert_eq!(
                ow % i128::from(p::DIMENSION_STEP),
                0,
                "{w}×{h} → {ow}×{oh} 宽不在网格上"
            );
            assert_eq!(
                oh % i128::from(p::DIMENSION_STEP),
                0,
                "{w}×{h} → {ow}×{oh} 高不在网格上"
            );
            assert!(
                ow * oh <= i128::from(cap),
                "{w}×{h} → {ow}×{oh} 超过上限 {cap}"
            );
        }
    }

    // snap_v5_preset 只会吐三个预设之一
    for (w, h) in [
        (1, 1),
        (1920, 1080),
        (1080, 1920),
        (1024, 1024),
        (5000, 64),
        (64, 5000),
    ] {
        let got = (
            p::snap_v5_preset(w, h).0 as i64,
            p::snap_v5_preset(w, h).1 as i64,
        );
        assert!(
            p::V5_NORMAL_PRESETS.contains(&got),
            "{w}×{h} 吸附成了 {got:?}，不在三个预设里"
        );
    }
}
