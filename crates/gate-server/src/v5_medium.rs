//! V5 Medium 降档（手动开关 `v5_medium_mode`：off / on）：此刻要不要把 V5 Full 请求改走 Medium。
//! 按余量自动切换的 `auto` 档站长 10-23 删除。

use crate::state::AppState;
use gate_core::site_flags::V5_MEDIUM_MODE;
use gate_core::v5_medium::Mode;

/// 站长设置的模式。读库失败按 `off`：降档开关出问题不能把出图拖垮。
pub async fn mode(state: &AppState) -> Mode {
    match state.db.get_setting(V5_MEDIUM_MODE.key).await {
        Ok(raw) => Mode::from_raw(raw.as_deref()),
        Err(e) => {
            tracing::warn!(error = %e, "读取 v5_medium_mode 失败，按 off 处理");
            Mode::Off
        }
    }
}

/// 出图热路径用：此刻要不要把 V5 Full 改走 Medium。
pub async fn active(state: &AppState) -> bool {
    mode(state).await == Mode::On
}
