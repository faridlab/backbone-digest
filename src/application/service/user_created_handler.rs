//! Consumer for the `sapiens.user.created` integration event — the
//! growth loop (R-DG10; hand-written; user-owned).
//!
//! Every newly-created INTERNAL user is subscribed to the configured
//! default digest when both config keys are set
//! (`digest.default_digest_id` on the builder + the host's
//! `digest.default_digest_emails` flag — the builder folds both into
//! [`UserCreatedHandler::default_digest`]).
//!
//! The trigger arrives on the HOST's integration bus (sapiens publishes
//! `UserDomainEvent::Created` → outbox → host relay → bus); this module
//! subscribes to nothing itself — the HOST registers this handler on
//! its bus (`register_handler(Arc::new(module.user_created_handler()))`).
//! There is NO sapiens Cargo edge: the payload is parsed as JSON, and
//! the internal-user predicate crosses through the fail-closed
//! recipient port.
//!
//! ## The internal predicate
//!
//! `sapiens.user.created`'s payload carries NO internal flag — the
//! predicate is an ACTIVE `organization_users` membership, resolved
//! through [`crate::application::service::engagement_port`]. Three
//! refusal shapes, none a silent skip:
//!
//! - **port unwired** → `EventError` (a composition bug the host must
//!   see, not absorb);
//! - **user not yet resolvable** → `EventError` (the relay redelivers;
//!   a race between the user row and the event is transient);
//! - **non-internal user** → audited `auto_subscribe_refused`
//!   (reason `non_internal`), NOT subscribed, delivery Ok — a
//!   legitimate non-subscription, recorded loudly.
//!
//! ## Idempotency (the at-least-once relay)
//!
//! The subscribe verb's `(digest, user)` unique key is the fence: a
//! replayed `sapiens.user.created` (the duplicate-create edge) re-runs
//! the upsert which no-ops on the existing row — `first_time` comes
//! back false and the `digest_user_auto_subscribed` audit fact fires
//! only on the first subscription, never duplicated by replays.

use async_trait::async_trait;
use backbone_messaging::{EventError, IntegrationEventEnvelope, IntegrationEventHandler};
use std::sync::Arc;
use uuid::Uuid;

use crate::application::service::digest_write_service::DigestWriteService;
use crate::application::service::engagement_port::{RecipientContextPort, RecipientContextSlot};

const HANDLER: &str = "digest.user_created";

/// The growth-loop consumer. Constructed by the module builder (carrying
/// the configured default digest) and registered on the HOST's bus.
pub struct UserCreatedHandler {
    write: Arc<DigestWriteService>,
    port: RecipientContextSlot,
    /// The `digest.default_digest_id` config; `None` = the growth loop
    /// is OFF (every event is refused-audited, never silently skipped).
    default_digest: Option<Uuid>,
}

impl UserCreatedHandler {
    pub fn new(
        write: Arc<DigestWriteService>,
        port: RecipientContextSlot,
        default_digest: Option<Uuid>,
    ) -> Self {
        Self { write, port, default_digest }
    }

    /// The event this handler subscribes to (exact match).
    pub const EVENT_TYPE: &'static str = "sapiens.user.created";
}

#[async_trait]
impl IntegrationEventHandler for UserCreatedHandler {
    async fn handle(&self, envelope: IntegrationEventEnvelope) -> Result<(), EventError> {
        let user_id: Uuid = serde_json::from_value(envelope.payload["user_id"].clone())
            .map_err(|e| {
                EventError::handler(
                    HANDLER,
                    format!("payload.user_id: {e} (envelope {})", envelope.id),
                )
            })?;

        // Growth loop off: audited refusal, never a silent skip.
        let Some(default_digest) = self.default_digest else {
            tracing::warn!(
                target: "digest::audit",
                event = "auto_subscribe_refused",
                reason = "default_digest_not_configured",
                user_id = %user_id,
                envelope_id = %envelope.id
            );
            return Ok(());
        };

        // The internal predicate — through the fail-closed port.
        let ctx = match self.port.resolve(user_id).await {
            Ok(ctx) => ctx,
            Err(e) if e.is_not_wired() => {
                // A composition bug: refuse LOUDLY as a delivery failure
                // so the host sees it on the relay.
                return Err(EventError::handler(
                    HANDLER,
                    format!("recipient port not wired; cannot resolve {user_id}: {e}"),
                ));
            }
            Err(e) => {
                // Unresolvable right now (row race, source trouble) —
                // error the delivery; the at-least-once relay retries.
                return Err(EventError::handler(
                    HANDLER,
                    format!("recipient {user_id} unresolvable: {e}"),
                ));
            }
        };

        if !ctx.is_internal {
            // External/portal user: a legitimate non-subscription —
            // audited, not subscribed, NOT a bus error.
            tracing::warn!(
                target: "digest::audit",
                event = "auto_subscribe_refused",
                reason = "non_internal",
                user_id = %user_id,
                envelope_id = %envelope.id
            );
            return Ok(());
        }

        // The subscribe verb — idempotent on the (digest, user) key.
        let first_time = self
            .write
            .subscribe_user(
                default_digest,
                user_id,
                Some(serde_json::json!({
                    "auto_subscribed": true,
                    "auto_subscribed_at": chrono::Utc::now().to_rfc3339(),
                    "auto_subscribed_envelope": envelope.id,
                })),
            )
            .await
            .map_err(|e| EventError::handler(HANDLER, format!("subscribe verb: {e}")))?;

        if first_time {
            tracing::info!(
                target: "digest::audit",
                event = "digest_user_auto_subscribed",
                digest_id = %default_digest,
                user_id = %user_id,
                envelope_id = %envelope.id
            );
        }
        // A replay (duplicate-create edge) lands here with first_time =
        // false: the row stays subscribed, no duplicate audit fact.

        Ok(())
    }

    fn event_patterns(&self) -> Vec<&'static str> {
        vec![Self::EVENT_TYPE]
    }

    fn name(&self) -> &'static str {
        "DigestUserCreatedHandler"
    }
}
