//! The fail-closed recipient-context port — the module's ONLY window onto
//! identity facts (hand-written; user-owned; see `metaphor.codegen.yaml`).
//!
//! The digest engines need four identity facts the module does not own:
//!
//! - **the recipient context** (email to mail to, active company — the
//!   render fence, last-login recency — the slowdown signal, and the
//!   internal-user predicate — the auto-subscribe gate);
//! - **connected-users counts** (the base KPI's source);
//! - **login recency across a digest's recipients** (the slowdown
//!   ladder's signal);
//! - **the recipient's group-key set** (the tip carousel's gate).
//!
//! backbone-sapiens owns all of that, and this module takes ZERO Cargo
//! edge onto it (the dep-edge substitution ruling): the facts cross as a
//! PORT. This module owns the trait; the host registers ONE
//! implementation. Two ship here:
//!
//! - [`RefusingRecipientContext`] — the deny-by-default tenant of the
//!   slot. Every call refuses with [`RecipientPortError::NotWired`]:
//!   until a host wires the port, auto-subscribe refuses LOUDLY (error
//!   `auto_subscribe_refused`), the slowdown ladder reports its signal
//!   unavailable (error `slowdown_signal_unavailable` — the sweep then
//!   holds the cadence rather than degrading on a guess), group-gated
//!   tips do NOT render (fail closed), and connected-users renders as
//!   source-unavailable. A missing composition is never a silent skip.
//! - [`SqlRecipientContext`] — the standard SQL adapter over the
//!   sapiens tables (public.users, sapiens.organization_users,
//!   public.user_roles / public.roles). No Cargo edge is needed to READ
//!   another module's schema over the shared pool; the host installs it
//!   via `DigestModule::set_recipient_port` (or composes its own).
//!
//! The internal-user predicate is **an active organization_users
//! membership** — the composition's ruling on Odoo's internal-user
//! notion. The slowdown signal is `users.last_login` directly (NOT a
//! login-log table — the cycle doc's identity.UserLog mapping re-pointed
//! onto the live column).

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

/// The identity facts for one recipient, resolved through the port.
#[derive(Debug, Clone)]
pub struct RecipientContext {
    pub user_id: Uuid,
    /// The email the digest mails to (sapiens users.email).
    pub email: String,
    /// The recipient's ACTIVE organization membership — the company
    /// fence the render computes under. `None` when the user holds no
    /// active membership (an out-of-company recipient: company-declared
    /// KPIs fail closed for them).
    pub company_id: Option<Uuid>,
    /// The internal-user predicate: TRUE iff an active organization_users
    /// membership exists. The auto-subscribe growth loop subscribes
    /// internal users ONLY.
    pub is_internal: bool,
    /// sapiens users.last_login — the slowdown ladder's signal column.
    pub last_login: Option<DateTime<Utc>>,
}

/// Why a port call failed.
#[derive(Debug, Clone, thiserror::Error)]
pub enum RecipientPortError {
    /// No [`RecipientContextPort`] is wired — the fail-closed refusal.
    /// Installing one via `DigestModule::set_recipient_port` is the only
    /// cure. Callers surface this as the loud refusals
    /// (`auto_subscribe_refused` / `slowdown_signal_unavailable`) or the
    /// fail-closed tip-gate skip — never a silent success.
    #[error("recipient context port not wired: {detail}")]
    NotWired { detail: String },
    /// The wired source failed (SQL trouble, missing table in this
    /// composition). Retryable at the caller's cadence.
    #[error("recipient context source failed: {0}")]
    Unavailable(String),
}

impl RecipientPortError {
    /// `true` when this is the unwired-refusal shape (callers choose
    /// their loud-refusal mapping on it).
    pub fn is_not_wired(&self) -> bool {
        matches!(self, RecipientPortError::NotWired { .. })
    }
}

/// The recipient-context seam. ONE implementation is composed by the
/// host; every identity fact the digest engines touch flows through it.
#[async_trait]
pub trait RecipientContextPort: Send + Sync {
    /// Resolve one recipient's identity facts. Auto-subscribe uses the
    /// internal flag, the render uses email + company fence, the ladder
    /// uses last_login.
    async fn resolve(&self, user_id: Uuid) -> Result<RecipientContext, RecipientPortError>;

    /// Count users of ONE company whose `last_login` falls in
    /// `[since, until)` — the Connected Users base KPI's source.
    async fn count_connected(
        &self,
        company_id: Uuid,
        since: DateTime<Utc>,
        until: DateTime<Utc>,
    ) -> Result<i64, RecipientPortError>;

    /// Did ANY of these users log in since `since`? — the slowdown
    /// ladder's signal (a digest with no engaged recipient degrades one
    /// rung on the CRON path only).
    async fn any_logged_in_since(
        &self,
        user_ids: &[Uuid],
        since: DateTime<Utc>,
    ) -> Result<bool, RecipientPortError>;

    /// The recipient's stable group-key set (sapiens role names in the
    /// SQL adapter) — the tip carousel's `group_key` gate. A tip whose
    /// group_key is not in this set does not render for the recipient.
    async fn group_keys(&self, user_id: Uuid) -> Result<HashSet<String>, RecipientPortError>;
}

/// The deny-by-default implementation: the slot's initial tenant.
pub struct RefusingRecipientContext;

fn not_wired(what: &str) -> RecipientPortError {
    RecipientPortError::NotWired {
        detail: format!(
            "no RecipientContextPort is wired; refusing {what} — compose one via \
             DigestModule::set_recipient_port (SqlRecipientContext is the standard adapter)"
        ),
    }
}

#[async_trait]
impl RecipientContextPort for RefusingRecipientContext {
    async fn resolve(&self, _user_id: Uuid) -> Result<RecipientContext, RecipientPortError> {
        Err(not_wired("recipient resolution"))
    }

    async fn count_connected(
        &self,
        _company_id: Uuid,
        _since: DateTime<Utc>,
        _until: DateTime<Utc>,
    ) -> Result<i64, RecipientPortError> {
        Err(not_wired("the connected-users count"))
    }

    async fn any_logged_in_since(
        &self,
        _user_ids: &[Uuid],
        _since: DateTime<Utc>,
    ) -> Result<bool, RecipientPortError> {
        Err(not_wired("the slowdown login signal"))
    }

    async fn group_keys(&self, _user_id: Uuid) -> Result<HashSet<String>, RecipientPortError> {
        Err(not_wired("the tip group-key resolution"))
    }
}

/// The recording test double: answers are canned per method.
pub struct CannedRecipientContext {
    pub resolved: std::sync::Mutex<Vec<Uuid>>,
    pub answer: std::sync::Mutex<CannedAnswers>,
}

#[derive(Default, Clone)]
pub struct CannedAnswers {
    pub context: Option<RecipientContext>,
    pub connected: i64,
    pub any_logged_in: bool,
    pub keys: HashSet<String>,
}

impl CannedRecipientContext {
    pub fn with(answer: CannedAnswers) -> Self {
        Self {
            resolved: std::sync::Mutex::new(Vec::new()),
            answer: std::sync::Mutex::new(answer),
        }
    }
}

#[async_trait]
impl RecipientContextPort for CannedRecipientContext {
    async fn resolve(&self, user_id: Uuid) -> Result<RecipientContext, RecipientPortError> {
        self.resolved.lock().unwrap_or_else(|e| e.into_inner()).push(user_id);
        self.answer
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .context
            .clone()
            .ok_or_else(|| RecipientPortError::Unavailable("no canned context".into()))
    }

    async fn count_connected(
        &self,
        _company_id: Uuid,
        _since: DateTime<Utc>,
        _until: DateTime<Utc>,
    ) -> Result<i64, RecipientPortError> {
        Ok(self.answer.lock().unwrap_or_else(|e| e.into_inner()).connected)
    }

    async fn any_logged_in_since(
        &self,
        _user_ids: &[Uuid],
        _since: DateTime<Utc>,
    ) -> Result<bool, RecipientPortError> {
        Ok(self.answer.lock().unwrap_or_else(|e| e.into_inner()).any_logged_in)
    }

    async fn group_keys(&self, _user_id: Uuid) -> Result<HashSet<String>, RecipientPortError> {
        Ok(self.answer.lock().unwrap_or_else(|e| e.into_inner()).keys.clone())
    }
}

/// A shared, swappable port slot — how the host registers its
/// composition (the survey CertificationGrantSlot precedent). The
/// engines are built over the slot defaulting to
/// [`RefusingRecipientContext`]; `DigestModule::set_recipient_port`
/// installs the host's implementation and every caller sees it on the
/// NEXT call.
#[derive(Clone)]
pub struct RecipientContextSlot {
    inner: Arc<std::sync::RwLock<Arc<dyn RecipientContextPort>>>,
}

impl RecipientContextSlot {
    pub fn install(&self, port: Arc<dyn RecipientContextPort>) {
        // A poisoned lock still holds the old value — recovering it beats
        // panicking every future call over one panicked writer.
        *self.inner.write().unwrap_or_else(|e| e.into_inner()) = port;
    }

    pub fn current(&self) -> Arc<dyn RecipientContextPort> {
        self.inner.read().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

impl Default for RecipientContextSlot {
    fn default() -> Self {
        Self {
            inner: Arc::new(std::sync::RwLock::new(Arc::new(RefusingRecipientContext))),
        }
    }
}

#[async_trait]
impl RecipientContextPort for RecipientContextSlot {
    async fn resolve(&self, user_id: Uuid) -> Result<RecipientContext, RecipientPortError> {
        self.current().resolve(user_id).await
    }

    async fn count_connected(
        &self,
        company_id: Uuid,
        since: DateTime<Utc>,
        until: DateTime<Utc>,
    ) -> Result<i64, RecipientPortError> {
        self.current().count_connected(company_id, since, until).await
    }

    async fn any_logged_in_since(
        &self,
        user_ids: &[Uuid],
        since: DateTime<Utc>,
    ) -> Result<bool, RecipientPortError> {
        self.current().any_logged_in_since(user_ids, since).await
    }

    async fn group_keys(&self, user_id: Uuid) -> Result<HashSet<String>, RecipientPortError> {
        self.current().group_keys(user_id).await
    }
}

/// The standard SQL adapter over the sapiens tables (host-installed; no
/// Cargo edge — reading another module's schema over the shared pool is
/// the family's cross-module read shape):
///
/// - `public.users` — email, last_login, status, soft-delete;
/// - `sapiens.organization_users` — the active-membership predicate
///   (internal-user) and the active-company resolution;
/// - `public.user_roles` + `public.roles` — the group-key set (role
///   names, the stable uppercase keys).
pub struct SqlRecipientContext {
    pool: PgPool,
}

impl SqlRecipientContext {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl RecipientContextPort for SqlRecipientContext {
    async fn resolve(&self, user_id: Uuid) -> Result<RecipientContext, RecipientPortError> {
        let row = sqlx::query_as::<_, (String, Option<DateTime<Utc>>, Option<Uuid>)>(
            r#"SELECT u.email, u.last_login,
                      (SELECT ou.organization_id
                       FROM sapiens.organization_users ou
                       WHERE ou.user_id = u.id AND ou.status = 'active'
                         AND (ou.metadata->>'deleted_at') IS NULL
                       ORDER BY ou.joined_at
                       LIMIT 1) AS company_id
               FROM users u
               WHERE u.id = $1 AND (u.metadata->>'deleted_at') IS NULL"#,
        )
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| RecipientPortError::Unavailable(e.to_string()))?;
        let Some((email, last_login, company_id)) = row else {
            return Err(RecipientPortError::Unavailable(format!(
                "user {user_id} not found (or soft-deleted) in sapiens"
            )));
        };
        Ok(RecipientContext {
            user_id,
            email,
            company_id,
            is_internal: company_id.is_some(),
            last_login,
        })
    }

    async fn count_connected(
        &self,
        company_id: Uuid,
        since: DateTime<Utc>,
        until: DateTime<Utc>,
    ) -> Result<i64, RecipientPortError> {
        let (n,) = sqlx::query_as::<_, (i64,)>(
            r#"SELECT count(DISTINCT u.id)
               FROM users u
               JOIN sapiens.organization_users ou
                 ON ou.user_id = u.id
                AND ou.status = 'active'
                AND (ou.metadata->>'deleted_at') IS NULL
                AND ou.organization_id = $1
               WHERE u.last_login >= $2 AND u.last_login < $3
                 AND u.status = 'active'
                 AND (u.metadata->>'deleted_at') IS NULL
                 AND u.status <> 'suspended'
                 AND u.status <> 'pending_verification'"#,
        )
        .bind(company_id)
        .bind(since)
        .bind(until)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| RecipientPortError::Unavailable(e.to_string()))?;
        Ok(n)
    }

    async fn any_logged_in_since(
        &self,
        user_ids: &[Uuid],
        since: DateTime<Utc>,
    ) -> Result<bool, RecipientPortError> {
        if user_ids.is_empty() {
            // No recipients at all is not a signal failure — but a digest
            // with zero active recipients has nobody engaged; the caller
            // (the ladder) treats empty as "no login" by construction.
            return Ok(false);
        }
        let (n,) = sqlx::query_as::<_, (i64,)>(
            r#"SELECT count(*) FROM users
               WHERE id = ANY($1)
                 AND last_login IS NOT NULL
                 AND last_login >= $2"#,
        )
        .bind(user_ids)
        .bind(since)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| RecipientPortError::Unavailable(e.to_string()))?;
        Ok(n > 0)
    }

    async fn group_keys(&self, user_id: Uuid) -> Result<HashSet<String>, RecipientPortError> {
        let rows = sqlx::query_as::<_, (String,)>(
            r#"SELECT r.name
               FROM roles r
               JOIN user_roles ur ON ur.role_id = r.id
               WHERE ur.user_id = $1
                 AND (r.metadata->>'deleted_at') IS NULL"#,
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| RecipientPortError::Unavailable(e.to_string()))?;
        Ok(rows.into_iter().map(|(name,)| name).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn unwired_port_refuses_loudly_everywhere() {
        let slot = RecipientContextSlot::default();
        let u = Uuid::new_v4();
        assert!(slot.resolve(u).await.unwrap_err().is_not_wired());
        assert!(slot
            .count_connected(u, Utc::now(), Utc::now())
            .await
            .unwrap_err()
            .is_not_wired());
        assert!(slot
            .any_logged_in_since(&[u], Utc::now())
            .await
            .unwrap_err()
            .is_not_wired());
        assert!(slot.group_keys(u).await.unwrap_err().is_not_wired());
    }

    #[tokio::test]
    async fn install_replaces_the_refusal() {
        let slot = RecipientContextSlot::default();
        slot.install(Arc::new(CannedRecipientContext::with(CannedAnswers {
            context: Some(RecipientContext {
                user_id: Uuid::new_v4(),
                email: "someone@example.test".into(),
                company_id: Some(Uuid::new_v4()),
                is_internal: true,
                last_login: None,
            }),
            ..Default::default()
        })));
        assert!(slot.resolve(Uuid::new_v4()).await.is_ok());
    }
}
