//! 两个集成测试共用的 [`TestHooks`]（Hooks 的内存实现）：记录事件 / 起点 / 冷却变化，
//! V5 与出图计数按 Token 累加，`block` 和 `post` 可以按用例调整。
#![allow(dead_code)]

use bytes::Bytes;
use gate_upstream::client::{BoxFuture, HookResult, Hooks};
use gate_upstream::pool::Cooldown;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Default, Clone)]
pub struct TestHooks(pub Arc<Inner>);

#[derive(Default)]
pub struct Inner {
    pub v5: Mutex<HashMap<String, i64>>,
    pub images: Mutex<HashMap<String, i64>>,
    pub events: Mutex<Vec<String>>,
    pub block: Mutex<Option<(String, String)>>,
    pub cooldowns: Mutex<Vec<(String, Cooldown)>>,
    /// 出图后静默 (最少, 最多)；默认 0（不静默）。
    pub post: Mutex<(f64, f64)>,
}

impl Hooks for TestHooks {
    fn day(&self) -> String {
        "2026-10-14".into()
    }
    fn now(&self) -> f64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs_f64()
    }
    fn event(&self, kind: &str, _message: &str, _cooldown: f64) {
        self.0.events.lock().unwrap().push(kind.to_string());
    }
    fn cooldown_changed(&self, token_id: &str, cooldown: Cooldown) {
        self.0
            .cooldowns
            .lock()
            .unwrap()
            .push((token_id.to_string(), cooldown));
    }
    fn has_guard(&self) -> bool {
        true
    }
    fn token_block_reason<'a>(
        &'a self,
        _t: &'a str,
        _d: &'a str,
    ) -> BoxFuture<'a, HookResult<Option<(String, String)>>> {
        let r = self.0.block.lock().unwrap().clone();
        Box::pin(async move { Ok(r) })
    }
    fn post_jitter(&self) -> (f64, f64) {
        *self.0.post.lock().unwrap()
    }
    fn upstream_v5_counter<'a>(
        &'a self,
        t: &'a str,
        _d: &'a str,
    ) -> BoxFuture<'a, HookResult<i64>> {
        let n = *self.0.v5.lock().unwrap().get(t).unwrap_or(&0);
        Box::pin(async move { Ok(n) })
    }
    fn bump_upstream_v5<'a>(&'a self, t: &'a str, _d: &'a str) -> BoxFuture<'a, HookResult<()>> {
        *self.0.v5.lock().unwrap().entry(t.to_string()).or_insert(0) += 1;
        Box::pin(async { Ok(()) })
    }
    fn bump_upstream_images<'a>(
        &'a self,
        t: &'a str,
        _d: &'a str,
        n: i64,
    ) -> BoxFuture<'a, HookResult<()>> {
        *self
            .0
            .images
            .lock()
            .unwrap()
            .entry(t.to_string())
            .or_insert(0) += n;
        Box::pin(async { Ok(()) })
    }
    fn validate_image_result(&self, body: Bytes, _expected: usize) -> BoxFuture<'static, bool> {
        Box::pin(async move { body.starts_with(b"PK") })
    }
}
