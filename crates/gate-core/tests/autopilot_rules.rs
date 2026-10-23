//! 自动驾驶两条规则（全站熔断、单 Key 限流）的手写测试：边界表 + 不变量（proptest）。
//!
//! 处罚类规则最容易出事，所以除了边界，还用随机输入守住几条不许破的性质：
//! 失败来自不到 2 把 Key 永不熔断；熔断一定是停 300 秒；已暂停的 Key 不重复暂停。
//! （每个拒绝原因码都必须明确归类的检查在 `reasons.rs`。）

use gate_core::autopilot::{
    self, BREAKER_SECS, BREAKER_UNTIL_KEY, BreakerInputs, BreakerRows, KEY_GUARD_PAUSE_REASON,
    KEY_GUARD_PAUSE_SECS, KeyGuardInputs,
};
use gate_core::pyc;
use gate_core::quota::Effect;
use proptest::prelude::*;
use std::collections::{HashMap, HashSet};

const NOW: f64 = 1_791_950_400.0;

fn breaker_in(fails: i64, total: i64, fail_keys: i64) -> BreakerInputs {
    BreakerInputs {
        now: NOW,
        rows: BreakerRows {
            fails: Some(fails),
            total: Some(total),
            fail_keys,
        },
        breaker_until: 0.0,
    }
}

// ---------------------------------------------------------------- 全站熔断

#[test]
fn breaker_boundary_table() {
    // (失败, 总数, 失败来自几把 Key) → 是否触发
    for (fails, total, keys, trip) in [
        (4, 10, 2, false), // 不到 5 次
        (5, 10, 2, true),  // 5 次、50%
        (5, 17, 2, false), // 29.4% < 30%
        (5, 16, 2, true),  // 31.25%
        (5, 0, 2, false),  // 总数 0：不除零、不触发
        (9, 10, 1, false), // 只来自 1 把 Key
        (9, 10, 0, false), // 只来自测试 Key（不算）
        (9, 10, 5, true),
    ] {
        let out = autopilot::breaker(&breaker_in(fails, total, keys));
        assert_eq!(
            (out.trip, out.applied),
            (trip, trip),
            "({fails},{total},{keys})"
        );
        assert_eq!(out.effects.is_empty(), !trip, "({fails},{total},{keys})");
    }
}

#[test]
fn breaker_texts_are_unchanged() {
    let out = autopilot::breaker(&breaker_in(2, 40, 2));
    assert_eq!(out.why, "最近 15 分钟上游失败 2/40：正常");
    assert_eq!(out.summary, out.why);
    for keys in [0, 1] {
        let out = autopilot::breaker(&breaker_in(9, 10, keys));
        assert_eq!(
            out.why,
            format!(
                "最近 15 分钟上游失败 9/10，只来自 {keys} 把 Key（多半是这把 Key 的请求有问题）：不熔断"
            )
        );
    }
    let out = autopilot::breaker(&breaker_in(8, 10, 2));
    assert_eq!(out.why, "最近 15 分钟上游失败 8/10：暂停出图 5 分钟");
    assert_eq!(
        out.effects,
        vec![
            Effect::TripBreaker {
                seconds: 300,
                reason:
                    "上游连续出错，已暂停出图 5 分钟（最近 15 分钟上游失败 8/10：暂停出图 5 分钟）"
                        .into(),
                now: NOW,
            },
            Effect::Set {
                key: BREAKER_UNTIL_KEY.into(),
                value: pyc::float_repr(NOW + 300.0),
            },
            Effect::Log {
                actor: "系统".into(),
                action: "自动驾驶：熔断".into(),
                target: String::new(),
                detail: "最近 15 分钟上游失败 8/10：暂停出图 5 分钟；全站暂停出图 5 分钟".into(),
            },
        ]
    );
}

#[test]
fn breaker_does_not_trip_twice() {
    // 已经在熔断中：判定照旧，但不再写、不再记日志
    let mut inp = breaker_in(8, 10, 2);
    inp.breaker_until = NOW + 120.0;
    let out = autopilot::breaker(&inp);
    assert!(out.trip && !out.applied);
    assert!(out.effects.is_empty());
    assert_eq!(
        out.summary,
        "最近 15 分钟上游失败 8/10：暂停出图 5 分钟（已在熔断中，不重复触发）"
    );
    // 刚好到期（breaker_until == now）就可以再触发
    inp.breaker_until = NOW;
    assert!(autopilot::breaker(&inp).applied);
}

#[test]
fn breaker_with_no_rows_is_quiet() {
    // SUM 为 NULL（窗口里没有出图请求）
    let out = autopilot::breaker(&BreakerInputs {
        now: NOW,
        rows: BreakerRows::default(),
        breaker_until: 0.0,
    });
    assert!(!out.trip);
    assert_eq!(out.why, "最近 15 分钟上游失败 0/0：正常");
}

#[test]
fn breaker_window_starts_after_the_last_breaker() {
    assert_eq!(autopilot::breaker_since(NOW, None).unwrap(), NOW - 900.0);
    assert_eq!(
        autopilot::breaker_since(NOW, Some("")).unwrap(),
        NOW - 900.0
    );
    assert_eq!(
        autopilot::breaker_since(NOW, Some(&pyc::float_repr(NOW - 60.0))).unwrap(),
        NOW - 60.0
    );
    // 很久以前的熔断不影响
    assert_eq!(
        autopilot::breaker_since(NOW, Some("1000.0")).unwrap(),
        NOW - 900.0
    );
    // 读不懂：报错，服务层按没有失败处理
    assert!(autopilot::breaker_since(NOW, Some("not-a-number")).is_err());
}

proptest! {
    /// 失败来自不到 2 把非测试 Key 时，无论多少次失败、占比多高，永不熔断。
    #[test]
    fn fewer_than_two_keys_never_trip(fails in 0i64..10_000, extra in 0i64..10_000, keys in 0i64..2) {
        let out = autopilot::breaker(&breaker_in(fails, fails + extra, keys));
        prop_assert!(!out.trip);
        prop_assert!(out.effects.is_empty());
    }

    /// 触发时：内存熔断正好 300 秒，`autopilot_breaker_until` 正好写 now + 300，只记一条日志。
    #[test]
    fn a_trip_is_always_three_hundred_seconds(
        fails in 5i64..1000, extra in 0i64..1000, keys in 2i64..50, now in 1.0e9f64..2.0e9,
    ) {
        let mut inp = breaker_in(fails, fails + extra, keys);
        inp.now = now;
        let out = autopilot::breaker(&inp);
        let want = fails as f64 / (fails + extra) as f64 >= 0.3;
        prop_assert_eq!(out.trip, want);
        if out.trip {
            prop_assert_eq!(out.effects.len(), 3);
            let seconds = match &out.effects[0] {
                Effect::TripBreaker { seconds, now: at, .. } if *at == now => Some(*seconds),
                _ => None,
            };
            prop_assert_eq!(seconds, Some(BREAKER_SECS));
            prop_assert_eq!(BREAKER_SECS, 300);
            let until = Effect::Set {
                key: BREAKER_UNTIL_KEY.into(),
                value: pyc::float_repr(now + 300.0),
            };
            prop_assert_eq!(&out.effects[1], &until);
            let logs = out.effects.iter().filter(|e| matches!(e, Effect::Log { .. })).count();
            prop_assert_eq!(logs, 1);
        }
    }
}

// ---------------------------------------------------------------- 单 Key 限流

fn keys(ids: &[i64]) -> Vec<(i64, String)> {
    ids.iter().map(|i| (*i, format!("成员{i}"))).collect()
}

fn guard_in(rejects: &[(i64, i64)], ids: &[i64], paused: &[(i64, f64)]) -> KeyGuardInputs {
    KeyGuardInputs {
        now: NOW,
        rejects: rejects.to_vec(),
        keys: keys(ids),
        paused: paused.iter().copied().collect(),
    }
}

#[test]
fn key_guard_threshold_is_120() {
    let out = autopilot::key_guard(&guard_in(&[(1, 119), (2, 120), (3, 121)], &[1, 2, 3], &[]));
    let hit: Vec<i64> = out.decisions.iter().map(|d| d.key).collect();
    assert_eq!(hit, [2, 3]);
    assert_eq!(
        out.decisions[0].why,
        "1 小时内被拒绝 120 次（多半是客户端在死循环重试）：暂停 1 小时"
    );
    assert_eq!(out.summary, "暂停 2 把：成员2、成员3");
    assert_eq!(
        out.effects,
        vec![
            Effect::PauseKey {
                key_id: 2,
                seconds: KEY_GUARD_PAUSE_SECS,
                reason: KEY_GUARD_PAUSE_REASON.into(),
                now: NOW,
            },
            Effect::Log {
                actor: "系统".into(),
                action: "自动驾驶：Key 限流".into(),
                target: "成员2".into(),
                detail: "1 小时内被拒绝 120 次（多半是客户端在死循环重试）：暂停 1 小时".into(),
            },
            Effect::PauseKey {
                key_id: 3,
                seconds: KEY_GUARD_PAUSE_SECS,
                reason: KEY_GUARD_PAUSE_REASON.into(),
                now: NOW,
            },
            Effect::Log {
                actor: "系统".into(),
                action: "自动驾驶：Key 限流".into(),
                target: "成员3".into(),
                detail: "1 小时内被拒绝 121 次（多半是客户端在死循环重试）：暂停 1 小时".into(),
            },
        ]
    );
    assert_eq!(KEY_GUARD_PAUSE_SECS, 3600);
}

#[test]
fn nothing_triggered() {
    let out = autopilot::key_guard(&guard_in(&[(1, 10)], &[1, 2], &[]));
    assert!(out.decisions.is_empty() && out.effects.is_empty());
    assert_eq!(out.summary, "没有 Key 触发");
}

#[test]
fn already_paused_keys_are_not_paused_again() {
    // 2 还在暂停中（到期时间在未来），3 的暂停已经过期
    let out = autopilot::key_guard(&guard_in(
        &[(1, 160), (2, 160), (3, 160)],
        &[1, 2, 3],
        &[(2, NOW + 600.0), (3, NOW - 1.0)],
    ));
    let applied: Vec<(i64, bool)> = out.decisions.iter().map(|d| (d.key, d.applied)).collect();
    assert_eq!(applied, [(1, true), (2, false), (3, true)]);
    let paused: Vec<i64> = out
        .effects
        .iter()
        .filter_map(|e| match e {
            Effect::PauseKey { key_id, .. } => Some(*key_id),
            _ => None,
        })
        .collect();
    assert_eq!(paused, [1, 3]);
    assert_eq!(out.summary, "暂停 2 把：成员1、成员3；另有 1 把已在暂停中");
    let all_paused = autopilot::key_guard(&guard_in(&[(2, 160)], &[2], &[(2, NOW + 1.0)]));
    assert_eq!(all_paused.summary, "1 把 Key 触发，都已在暂停中");
    assert!(all_paused.effects.is_empty());
}

#[test]
fn rejects_of_keys_outside_the_list_do_not_count() {
    // 测试 / 站长 / 停用的 Key 不在 `keys` 里（查询就排除了）：被拒再多也不暂停
    let out = autopilot::key_guard(&guard_in(&[(9, 500)], &[1, 2], &[]));
    assert!(out.decisions.is_empty() && out.effects.is_empty());
}

#[test]
fn duplicate_ids_keep_the_first_position_and_the_last_name() {
    let inp = KeyGuardInputs {
        now: NOW,
        rejects: vec![(1, 140), (2, 140)],
        keys: vec![(2, "旧名".into()), (1, "一号".into()), (2, "新名".into())],
        paused: HashMap::new(),
    };
    let out = autopilot::key_guard(&inp);
    let order: Vec<(i64, &str)> = out
        .decisions
        .iter()
        .map(|d| (d.key, d.name.as_str()))
        .collect();
    assert_eq!(order, [(2, "新名"), (1, "一号")]);
}

proptest! {
    /// 暂停的一定是：在列表里、被拒 ≥ 120 次、此刻没在暂停中的 Key；顺序同列表；每把最多一次。
    #[test]
    fn only_eligible_keys_are_paused(
        rows in prop::collection::vec((0i64..20, 0i64..400), 0..30),
        ids in prop::collection::vec(0i64..20, 0..20),
        paused in prop::collection::vec((0i64..20, -100.0f64..100.0), 0..10),
    ) {
        let rejects: HashMap<i64, i64> = rows.iter().copied().collect();
        let paused_map: HashMap<i64, f64> = paused
            .iter()
            .map(|(k, d)| (*k, NOW + d))
            .collect();
        let inp = KeyGuardInputs {
            now: NOW,
            rejects: rows.clone(),
            keys: keys(&ids),
            paused: paused_map.clone(),
        };
        let out = autopilot::key_guard(&inp);
        let mut seen = HashSet::new();
        let mut expected = Vec::new();
        for id in &ids {
            if seen.insert(*id)
                && rejects.get(id).copied().unwrap_or(0) >= 120
                && paused_map.get(id).copied().unwrap_or(0.0) <= NOW
            {
                expected.push(*id);
            }
        }
        let got: Vec<i64> = out
            .effects
            .iter()
            .filter_map(|e| match e {
                Effect::PauseKey { key_id, seconds, .. } if *seconds == 3600 => Some(*key_id),
                Effect::PauseKey { .. } => Some(-1),
                _ => None,
            })
            .collect();
        prop_assert_eq!(got, expected);
    }
}
