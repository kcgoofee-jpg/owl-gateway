//! 已认证的 Key 凭据与统一的成员有效规则。
use gate_core::member_policy::MemberPolicy;
use gate_db::{Db, DbError};
use serde::Deserialize;
use serde_json::Value;

#[derive(Clone, Deserialize)]
struct Credential {
    id: i64,
    name: String,
    token: String,
    enabled: i64,
    expires_at: Option<f64>,
    discord_id: Option<String>,
    disabled_reason: Option<String>,
}

#[derive(Clone)]
pub struct Key {
    credential: Credential,
    pub policy: MemberPolicy,
    pub owner_image_full: bool,
    /// 已开通的赞助席。和站长号一样，不参加「用满后分享才能出图」。
    pub patron: bool,
    /// 站长号或赞助席插队：排在所有等待中的生图前面，并越过全站排队满。
    pub queue_jump: bool,
    /// 赞助席绑定的专属上游 Token；有则出图只走这把，不进全站池。
    pub bound_token_id: Option<String>,
}

impl std::fmt::Debug for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Key")
            .field("id", &self.id())
            .finish_non_exhaustive()
    }
}

impl Key {
    pub async fn load(db: &Db, row: Value) -> Result<Self, DbError> {
        let credential: Credential =
            serde_json::from_value(row).map_err(|_| DbError::Invalid("Key 凭据结构无效".into()))?;
        if credential.id <= 0 || !matches!(credential.enabled, 0 | 1) {
            return Err(DbError::Invalid("Key 凭据值无效".into()));
        }
        let (policy, owner_image_full, patron_on, queue_jump, bound_token_id) =
            match credential.discord_id.as_deref().filter(|s| !s.is_empty()) {
                Some(subject) => {
                    let snapshot = db.member_policy(subject).await?;
                    let owner_image_full = snapshot.owner_image.is_some_and(|p| p.enabled);
                    let patron = snapshot.patron.as_ref().filter(|p| p.enabled);
                    let patron_on = patron.is_some();
                    let bound_token_id = patron.and_then(|p| p.bound_token_id.clone());
                    let queue_jump = owner_image_full || patron.is_some_and(|p| p.queue_jump);
                    (
                        snapshot.effective,
                        owner_image_full,
                        patron_on,
                        queue_jump,
                        bound_token_id,
                    )
                }
                None => (db.member_defaults().await?, false, false, false, None),
            };
        Ok(Self {
            credential,
            policy,
            owner_image_full,
            patron: patron_on,
            queue_jump,
            bound_token_id,
        })
    }

    pub async fn refresh_policy(&self, db: &Db) -> Result<MemberPolicy, DbError> {
        if self.subject().is_empty() {
            return Err(DbError::Invalid("Key 尚未绑定成员".into()));
        }
        Ok(db.member_policy(self.subject()).await?.effective)
    }

    pub fn id(&self) -> i64 {
        self.credential.id
    }
    pub fn name(&self) -> &str {
        &self.credential.name
    }
    pub fn token(&self) -> &str {
        &self.credential.token
    }
    pub fn subject(&self) -> &str {
        self.credential.discord_id.as_deref().unwrap_or("")
    }
    pub fn enabled(&self) -> bool {
        self.credential.enabled == 1
    }
    pub fn disabled_reason(&self) -> Option<&str> {
        self.credential.disabled_reason.as_deref()
    }
    pub fn expires_at(&self) -> Option<f64> {
        self.credential.expires_at
    }
    pub fn expired(&self, now: f64) -> bool {
        self.expires_at().is_some_and(|e| e != 0.0 && e < now)
    }
    pub fn rpm(&self) -> i64 {
        i64::from(self.policy.rpm)
    }
    pub fn daily_images(&self) -> i64 {
        i64::from(self.policy.daily_images)
    }
    pub fn daily_v5(&self) -> i64 {
        i64::from(self.policy.daily_v5)
    }
    pub fn daily_anlas(&self) -> f64 {
        f64::from(self.policy.daily_anlas)
    }
    pub fn monthly_anlas(&self) -> f64 {
        self.policy.monthly_anlas
    }
    pub fn allows_paid_parameters(&self) -> bool {
        self.policy.paid_parameters
    }

    /// 专属捐赠账号，或已打开高规格开关：不再套免费档钳制。
    pub fn skip_free_clamp(&self) -> bool {
        self.allows_paid_parameters() || self.bound_token_id.is_some()
    }

    /// 挂费 V5：站长号、专属账号或已打开高规格开关。
    pub fn allows_paid_v5(&self) -> bool {
        self.owner_image_full || self.skip_free_clamp()
    }

    /// 专属 Token 的 Anlas / V5 不占全站池，也不受全站 V5 暂停。
    pub fn skip_site_budget(&self) -> bool {
        self.bound_token_id.is_some()
    }
}
