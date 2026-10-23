//! 拒绝原因码（移植自 `app/reasons.py`）：成员看到的是中文说明，统计 / 自动驾驶按这里的码数。
//!
//! 以前这些地方用 `detail LIKE '%排队的人太多%'` 数中文，改一个字就静默失效。现在 guard / 鉴权返回 [`Reason`]
//! （带 `.code` 的说明文字）→ `err(..., code)` → `usage_log.reason`。所有中文文案是面向成员的，逐字保持。

use crate::pyc;
use crate::pyext::is_nd;

pub const KEY_PAUSED: &str = "key_paused"; // Key 被自动驾驶临时暂停
pub const QUEUE_FULL: &str = "queue_full"; // 全站排队满（节约模式按它判断拥挤）
pub const KEY_BUSY: &str = "key_busy"; // 这把 Key 上一张还没出完（10-22 起改记 member_queue_full，只剩历史日志）
pub const MEMBER_QUEUE_FULL: &str = "member_queue_full"; // 这个成员排队中的图已到每人上限（guard `queue_per_member`）
/// 上游「并发锁」（NovelAI：Concurrent generation is locked）：这个账号的上一张还没释放。
/// 网关会短暂等待后在同一账号上重试；只有重试几次仍被锁才以这个码失败。不是限流，不冷却。
pub const UPSTREAM_LOCKED: &str = "upstream_locked";
pub const HOURLY_CAP: &str = "hourly_cap"; // 账号每小时上限（10-22 删除，不再产生；只剩历史日志）
pub const QUIET_CAP: &str = "quiet_cap"; // 安静时段的每小时上限
/// 账号 3 小时上限。这个功能已经删除，但 `usage_log.reason` 里还留着历史记录，
/// 短说明（`LABELS`）和单 Key 限流的排除名单（[`crate::autopilot::KEY_GUARD_SKIP_REASONS`]）仍然认它，
/// 所以常量保留（`reason_label("cap_3h")` 的结果依赖它）。
pub const CAP_3H: &str = "cap_3h";
/// 账号每 10 分钟突发窗口。这个功能 3.0 已删除（账号出图间隔下到不了），与 [`CAP_3H`] 一样只为
/// `usage_log.reason` 里的历史记录保留（短说明、回填、单 Key 限流的排除项都还认它）。
pub const BURST_CAP: &str = "burst_cap";
pub const DAILY_CAP: &str = "daily_cap"; // 账号每日上限
pub const PACED: &str = "paced"; // 全站每日上限按时段放行（10-22 删除，不再产生；只剩历史日志）
pub const BREAKER: &str = "breaker"; // 全站熔断
pub const COOLDOWN: &str = "cooldown"; // 上游 429 后全站暂停出图
pub const V5_EXHAUSTED: &str = "v5_exhausted"; // 账号 V5 免费额度用完（未开通 Anlas 的 Key 不能自动转付费）
pub const QUEUE_TIMEOUT: &str = "queue_timeout"; // 排队 / 预算核对等待超时（站点忙，不是成员的问题）
pub const MEMBER_QUOTA: &str = "member_quota"; // 这把 Key 自己的每日 / 每月额度用完
pub const SITE_QUOTA: &str = "site_quota"; // 全站共享额度（全站 V5 / 全站 Anlas / 月预算 / Vibe 池）用完
pub const KEY_RATE: &str = "key_rate"; // 这把 Key 的请求频率 / 在途请求数超限（客户端行为）
pub const BODY_ABORT: &str = "body_abort"; // 请求体传输中断（客户端或网络，不是上游）
pub const FEATURE_OFF: &str = "feature_off"; // 该功能没给这把 Key 开通 / 全局关闭（成员可控）
pub const TEST_REQUEST: &str = "test_request"; // 客户端的「连接测试」请求（尺寸很小、步数很少、提示词很短）：在网关挡下，不出图
pub const KEY_DISABLED: &str = "key_disabled"; // Key 已停用 / 已过期（站点层面的状态，单 Key 限流不再重复暂停它）
pub const KEY_INVALID: &str = "key_invalid"; // 没带 Key / Key 无效（没有 key_id，按 IP 拦截，不进单 Key 限流）
pub const PARAM: &str = "param"; // 参数不被支持（模型、采样器、角色数、权重写法……），成员按提示改就好

/// 带原因码的拒绝说明（旧版是 `str` 子类，旧调用方照常当字符串用）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reason {
    text: String,
    code: String,
}

impl Reason {
    pub fn new(text: impl Into<String>, code: &str) -> Self {
        Self {
            text: text.into(),
            code: code.to_string(),
        }
    }

    /// 没有原因码的普通说明（对应旧版里直接传 `str`）。
    pub fn plain(text: impl Into<String>) -> Self {
        Self::new(text, "")
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn code(&self) -> &str {
        &self.code
    }
}

impl std::ops::Deref for Reason {
    type Target = str;
    fn deref(&self) -> &str {
        &self.text
    }
}

impl std::fmt::Display for Reason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text)
    }
}

/// 只用于数据库升级时回填原因码上线之前的旧记录（按顺序匹配，先匹配到的生效；模式是 SQL `LIKE`）。
pub const BACKFILL: &[(&str, &str)] = &[
    (QUEUE_FULL, "%排队的人太多%"),
    (HOURLY_CAP, "%本小时出图量已达上限%"),
    (BURST_CAP, "%本时段出图已达上限%"),
    // 全站暂停出图的几种（熔断 / 上游限流冷却 / 安静时段）文案里也有「暂停」，
    // 所以 Key 暂停只认它自己的片段，不能用宽泛的「暂停」
    (BREAKER, "%NovelAI 连续出错%"),
    (BREAKER, "%上游连续出错%"),
    (COOLDOWN, "%全站暂停出图约%"),
    (COOLDOWN, "%全站出图要冷却%"),
    (QUIET_CAP, "%夜间安静时段%"),
    (DAILY_CAP, "%本站今天的出图总量用完了%"),
    (PACED, "%这个时段全站的出图份额用完了%"),
    (KEY_PAUSED, "%Key 已被临时暂停%"),
    (KEY_PAUSED, "%疑似客户端在反复重试%"),
    (KEY_PAUSED, "%Key 已被暂停%"),
    // 站点级在前：全站额度用完的文案里也含「额度已用完」，不能被个人额度那条先吃掉
    (SITE_QUOTA, "%全站今天的 V5 额度被用完了%"),
    (SITE_QUOTA, "%全站今天的 Vibe 额度被用完了%"),
    (SITE_QUOTA, "%全站今天的 Anlas 额度被用完了%"),
    (SITE_QUOTA, "%全站本月的 Anlas 预算用完了%"),
    (SITE_QUOTA, "%要消耗 Anlas，本站还没开放%"),
    (V5_EXHAUSTED, "%NovelAI 账号今天的 V5 免费额度用完了%"),
    (V5_EXHAUSTED, "%V5 暂停中%"),
    (MEMBER_QUOTA, "%你今天的 V4.5 免费额度用完了%"),
    (MEMBER_QUOTA, "%你今天的 V5 额度用完了%"),
    (MEMBER_QUOTA, "%你今天的 Anlas 额度不够%"),
    (MEMBER_QUOTA, "%你本月的 Anlas 额度不够%"),
    (MEMBER_QUOTA, "%你的 Key 只能免费出图%"),
    // 2.x / 早期 3.0 的额度文案（库里的老记录）
    (SITE_QUOTA, "%全站今日 V5 额度已用完%"),
    (SITE_QUOTA, "%全站今日 Vibe 额度已抢完%"),
    (SITE_QUOTA, "%全站今日 Anlas 额度已抢完%"),
    (SITE_QUOTA, "%全站本月 Anlas 预算已耗尽%"),
    (MEMBER_QUOTA, "%今日 V4.5 免费图额度已用完%"),
    (MEMBER_QUOTA, "%已达今日 V5 额度%"),
    (MEMBER_QUOTA, "%今日 Anlas 额度不足%"),
    (MEMBER_QUOTA, "%本月 Anlas 额度不足%"),
    (MEMBER_QUOTA, "%今日文本额度%"),
    (QUEUE_TIMEOUT, "%排队超时%"),
    (QUEUE_TIMEOUT, "%等待超时%"),
    (KEY_RATE, "%请求过于频繁%"),
    (KEY_RATE, "%同时进行中的请求过多%"),
    (BODY_ABORT, "%请求体传输中断%"),
    (BODY_ABORT, "%上传中断了%"),
    // 免费图日额（V5 那条在上面的 SITE_QUOTA 之后匹配，文案含「及以下」）
    (MEMBER_QUOTA, "%免费图额度已用完%"),
    (FEATURE_OFF, "%暂无权使用%"),
    (FEATURE_OFF, "%站长暂未开放此功能%"),
    (FEATURE_OFF, "%本站只提供免费出图%"),
    (KEY_BUSY, "%上一张图还没出完%"),
    (MEMBER_QUEUE_FULL, "%张在排队，等前面的出完再发%"),
    (UPSTREAM_LOCKED, "%上一张还没释放%"),
    // 功能权限：模型范围 / 图生图 / 白名单这类「没给这把 Key 开」的拒绝
    (FEATURE_OFF, "%模型权限：仅允许%"),
    (FEATURE_OFF, "%img2img 未开放%"),
    (FEATURE_OFF, "%未列入本站图片模型白名单%"),
    // 停用 / 过期的 Key：全站重置停掉的 / 站长停用的（3.0 文案）/ 2.x 的旧文案
    (KEY_DISABLED, "%全站重置时已经失效%"),
    (KEY_DISABLED, "%已被站长停用%"),
    (KEY_DISABLED, "%该 Key 已被禁用%"),
    (KEY_DISABLED, "%这把 Key 已过期%"),
    (KEY_INVALID, "%没有收到 API Key%"),
    (KEY_INVALID, "%这个 API Key 无效%"),
    (KEY_INVALID, "%用无效 Key 请求的次数太多%"),
    // 参数问题：文案千变万化，只能按几个明确的片段匹配（成员按提示改客户端设置即可）
    (PARAM, "%不支持 V4/V4.5/V5 模型%"),
    (PARAM, "%角色太多%"),
    (PARAM, "%异常权重%"),
    (PARAM, "%必须是非负整数%"),
    (PARAM, "%权重括号嵌套过深%"),
    (PARAM, "%缺少 model%"),
    (PARAM, "%不支持 Vibe%"),
    (PARAM, "%Vibe 编码只支持%"),
    (PARAM, "%精确参考的类型只能是%"),
    // 10-23 formal wording (site-level patterns first, same rule as above)
    (SITE_QUOTA, "%全站今日 Vibe 额度已用完%"),
    (SITE_QUOTA, "%全站今日 Anlas 额度已用完%"),
    (SITE_QUOTA, "%全站本月 Anlas 预算已用完%"),
    (SITE_QUOTA, "%需要消耗 Anlas，本站未开放%"),
    (V5_EXHAUSTED, "%V5 暂停，未扣费%"),
    (MEMBER_QUOTA, "%今日 V4.5 额度已用完%"),
    (MEMBER_QUOTA, "%今日 V5 额度已用完%"),
    (MEMBER_QUOTA, "%Key 仅限免费出图%"),
    (DAILY_CAP, "%本站今日出图总量已达上限%"),
    (QUEUE_FULL, "%全站排队已满%"),
    (MEMBER_QUEUE_FULL, "%张在排队，请等前面完成后再发%"),
    (UPSTREAM_LOCKED, "%上一张图仍未释放%"),
    (COOLDOWN, "%全站出图暂停冷却%"),
    (QUEUE_TIMEOUT, "%额度核对超时%"),
    (BODY_ABORT, "%上传中断，%"),
    (FEATURE_OFF, "%本站未开放此功能%"),
    (FEATURE_OFF, "%本站仅提供免费出图%"),
    (FEATURE_OFF, "%无权使用%"),
    (KEY_DISABLED, "%在全站重置时失效%"),
    (KEY_DISABLED, "%这把 Key 已被停用%"),
    (KEY_INVALID, "%未收到 API Key%"),
    (KEY_INVALID, "%API Key 无效%"),
    (KEY_INVALID, "%无效 Key 请求次数过多%"),
    (PARAM, "%角色数量超限%"),
];

/// 原因码 → 给站长看的短说明（`LABELS`，顺序同旧版）。
pub const LABELS: &[(&str, &str)] = &[
    (KEY_PAUSED, "Key 被暂停"),
    (QUEUE_FULL, "排队满"),
    (KEY_BUSY, "上一张没出完就重发"),
    (MEMBER_QUEUE_FULL, "每人排队已满"),
    (UPSTREAM_LOCKED, "上游并发锁（不算限流）"),
    (HOURLY_CAP, "本小时全站上限"),
    (BURST_CAP, "突发窗口限速"),
    (QUIET_CAP, "安静时段上限"),
    (CAP_3H, "3 小时全站上限"),
    (DAILY_CAP, "全站每日上限"),
    (PACED, "全天平滑放行"),
    (BREAKER, "全站熔断"),
    (COOLDOWN, "上游限流暂停"),
    (V5_EXHAUSTED, "全站 V5 用完"),
    (QUEUE_TIMEOUT, "排队超时"),
    (MEMBER_QUOTA, "个人额度用完"),
    (SITE_QUOTA, "全站额度用完"),
    (KEY_RATE, "Key 请求过快"),
    (BODY_ABORT, "请求体中断"),
    (TEST_REQUEST, "连接测试（未出图）"),
    (KEY_DISABLED, "Key 已停用"),
    (KEY_INVALID, "Key 无效"),
];

/// `^\d{3}\s*`：开头三个十进制数字（HTTP 状态码）加后面的空白。
fn strip_status_prefix(detail: &str) -> &str {
    let mut it = detail.char_indices();
    for _ in 0..3 {
        match it.next() {
            Some((_, c)) if is_nd(c) => {}
            _ => return detail,
        }
    }
    let rest = it.next().map_or("", |(i, _)| &detail[i..]);
    rest.trim_start_matches(pyc::is_space)
}

/// 没带原因码的拒绝按说明文字归类（与 [`BACKFILL`] 同一张表）：写日志时兜底，
/// 这样同一种拒绝不会因为走了没带码的路径、要等下次重启回填才有原因码。认不出返回空串。
pub fn classify(detail: &str) -> &'static str {
    BACKFILL
        .iter()
        .find(|(_, pat)| detail.contains(pat.trim_matches('%')))
        .map_or("", |(code, _)| code)
}

/// 拒绝原因的短说明：有原因码用码，没有就从 detail 里取第一句（去掉开头的 HTTP 状态码）。
pub fn reason_label(code: &str, detail: &str) -> String {
    if !code.is_empty()
        && let Some((_, label)) = LABELS.iter().find(|(c, _)| *c == code)
    {
        return (*label).to_string();
    }
    let text = pyc::strip(strip_status_prefix(detail));
    // re.split(r"[（(，,。：:；;]", text, maxsplit=1)[0].strip()
    let first = text
        .split(['（', '(', '，', ',', '。', '：', ':', '；', ';'])
        .next()
        .unwrap_or("");
    let text = pyc::strip(first);
    if pyc::len_chars(text) > 18 {
        format!("{}…", pyc::take_chars(text, 18))
    } else if text.is_empty() {
        "其他".to_string()
    } else {
        text.to_string()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn classify_matches_backfill_patterns() {
        assert_eq!(
            classify("提示词里有异常权重 2025::（多半是画师名里的数字紧挨着 ::），没有扣费"),
            PARAM
        );
        assert_eq!(
            classify(
                "429 等 NovelAI 出图时排队超时（站点正忙，不是你的问题），没有扣费。请稍后再试"
            ),
            QUEUE_TIMEOUT
        );
        assert_eq!(classify("未列入本站图片模型白名单"), FEATURE_OFF);
        // 站点级在前：全站额度的文案不能被个人额度那条先吃掉
        assert_eq!(
            classify("全站今日 V5 额度已用完，及以下免费图额度已用完"),
            SITE_QUOTA
        );
        // 现行额度文案
        assert_eq!(
            classify("全站今天的 V5 额度被用完了（全站每天 300 张，先到先得），没有扣费"),
            SITE_QUOTA
        );
        assert_eq!(
            classify("你今天的 V5 额度用完了（每天 20 张），没有扣费"),
            MEMBER_QUOTA
        );
        // 全站暂停出图不是 Key 暂停
        assert_eq!(
            classify("现在是夜间安静时段（3:00–7:00），本站暂停出图，没有扣费。7 点后再来"),
            QUIET_CAP
        );
        assert_eq!(
            classify("NovelAI 连续出错，本站暂停出图，没有扣费。约 60 秒后自动恢复，请稍后再试"),
            BREAKER
        );
        assert_eq!(
            classify("NovelAI 那边正在限流，全站暂停出图约 30 秒，没有扣费。请稍后再试"),
            COOLDOWN
        );
        assert_eq!(
            classify("这把 Key 已被临时暂停，没有扣费，10-21 20:00 自动恢复。"),
            KEY_PAUSED
        );
        // Disabled keys and client parameter errors seen without a code on 10-21
        assert_eq!(
            classify(crate::key_status::disabled_message(Some(
                crate::key_status::SITE_RESET
            ))),
            KEY_DISABLED
        );
        assert_eq!(
            classify(crate::key_status::disabled_message(None)),
            KEY_DISABLED
        );
        assert_eq!(classify("403 该 Key 已被禁用"), KEY_DISABLED);
        assert_eq!(classify("缺少 model"), PARAM);
        assert_eq!(
            classify(
                "400 这个模型不支持 Vibe，没有扣费。请换成 V3、V4 或 V4.5，或去掉 Vibe 参考图"
            ),
            PARAM
        );
        assert_eq!(
            classify("400 Vibe 编码只支持 V4 / V4.5 模型，没有扣费"),
            PARAM
        );
        assert_eq!(
            classify("400 精确参考的类型只能是「角色」「风格」或「角色和风格」，没有扣费"),
            PARAM
        );
        assert_eq!(classify(crate::quota::V5_PAUSED_TEXT), V5_EXHAUSTED);
        assert_eq!(
            classify("全站今日 V5 额度已用完，每天 300 张，未扣费。"),
            SITE_QUOTA
        );
        assert_eq!(
            classify("今日 V5 额度已用完，每天 20 张，未扣费。"),
            MEMBER_QUOTA
        );
        assert_eq!(
            classify("已有 2 张在排队，请等前面完成后再发。"),
            MEMBER_QUEUE_FULL
        );
        assert_eq!(
            classify("全站排队已满，最多同时排 4 张，未扣费。"),
            QUEUE_FULL
        );
        assert_eq!(classify("upstream 500（nai-diffusion-5-full）"), "");
        assert!(BACKFILL.iter().all(|(_, p)| p.starts_with('%')
            && p.ends_with('%')
            && !p[1..p.len() - 1].contains('%')));
    }

    use super::*;
}
