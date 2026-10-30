//! Scheduled orchestration: load one database snapshot, evaluate pure rules, persist reviewable evidence.
use crate::{asn::AsnDb, ban_link, jobs::JobError, state::AppState};
use gate_core::abuse::{self, NetworkClass, ScanWindow};
use std::collections::HashMap;
const ASN_FILE: &str = "ip2asn-combined.tsv.gz";
pub async fn run(state: &AppState) -> Result<String, JobError> {
    let now = state.now();
    let gate_since = state
        .db
        .get_setting(crate::auth::WEB_LOGIN_SINCE_KEY)
        .await?
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0.0);
    let asn_path = state.db.path().parent().map(|d| d.join(ASN_FILE));
    let asn = match asn_path.filter(|p| p.exists()) {
        Some(path) => tokio::task::spawn_blocking(move || AsnDb::load(&path).ok())
            .await
            .ok()
            .flatten(),
        None => None,
    };
    let snapshot = state
        .db
        .abuse_scan_snapshot(ScanWindow::new(now, gate_since))
        .await?;
    let clock = state.clock.clone();
    let mut official = vec!["https://novelai.net".to_string()];
    let site = crate::client_hints::origin_only(&state.env.site_url);
    if !site.is_empty() {
        official.push(site);
    }
    let outcome = tokio::task::spawn_blocking(move || {
        let lookup = |ip: &str| {
            asn.as_ref()
                .and_then(|db| db.lookup(ip))
                .map(|entry| NetworkClass {
                    asn: entry.asn,
                    hosting: entry.hosting,
                })
        };
        let network = asn
            .as_ref()
            .map(|_| &lookup as &dyn Fn(&str) -> Option<NetworkClass>);
        abuse::evaluate(&snapshot, now, &clock, &official, network)
    })
    .await
    .map_err(|error| JobError::new("ScanTask", error.to_string()))?;
    let notes = outcome.notes;
    let signals = outcome.signals;
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for s in &signals {
        *counts.entry(s.kind).or_default() += 1;
        let prev = match s.alert {
            Some(_) => state.db.abuse_signal_status(s.kind, &s.subject).await?,
            None => None,
        };
        let id = state
            .db
            .upsert_abuse_signal(
                s.kind,
                &s.subject,
                &s.key_ids,
                s.score,
                &s.detail.to_string(),
                now,
            )
            .await?;
        if s.all_banned && state.db.confirm_open_abuse_signal(id).await? {
            *counts.entry("全已封禁").or_default() += 1;
        }
        if let Some(text) = &s.alert {
            let after = state.db.abuse_signal_status(s.kind, &s.subject).await?;
            ban_link::alert_if_opened(state, prev.as_deref(), after.as_deref(), text);
        }
    }
    // 这一轮没再出现的（比如证据过了 14 天窗口、规则改严了），而且站长还没处理过的：标成过期，记录留着
    let expired = state.db.expire_stale_open_abuse_signals(now).await?;
    if expired > 0 {
        *counts.entry("过期").or_default() += expired;
    }
    let mut parts: Vec<String> = counts.iter().map(|(k, n)| format!("{k} {n}")).collect();
    parts.sort();
    let head = if parts.is_empty() {
        "没有新的可疑信号".to_string()
    } else {
        format!("可疑信号：{}", parts.join("，"))
    };
    // 只记录、不提示的那几项：设备簇规模 + 链路健康度（站长 10-31：同 IP 多账号没有意义）
    let mut tail = format!(
        "设备簇 {} 个（{} 个账号）、没交指纹 {}",
        notes.device_clusters, notes.device_cluster_accounts, notes.fp_missing
    );
    if notes.same_ip_switches > 0 {
        tail.push_str(&format!("、同网换号 {}（不提示）", notes.same_ip_switches));
    }
    Ok(format!("{head}｜{tail}"))
}
