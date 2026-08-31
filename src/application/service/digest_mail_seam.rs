//! The outbound mail seam (hand-written; user-owned).
//!
//! The module never writes mail rows by hand: every digest mail crosses
//! backbone-mail's PUBLIC services — `message_post` (empty recipients —
//! mints the message row only) then `MailQueueWriteService::enqueue` with
//! the per-mail custom headers (mail v0.2.7's `headers` parameter, 5th
//! positional, `Option<&serde_json::Value>`; CRLF is refused at enqueue —
//! the header-injection guard runs BEFORE any write).
//!
//! The trait exists so the cron's failure policy
//! (`MailDeliveryException` → warning, no advance) and the probes have a
//! seam to inject at; [`DefaultMailSeam`] is the only real implementation
//! and [`RecordingMailSeam`] the test double (in-file, the survey
//! RecordingEventSink precedent).

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;

use crate::application::service::digest_error::DigestError;

/// One outgoing digest mail as the render produced it.
#[derive(Debug, Clone)]
pub struct OutgoingDigestMail {
    pub digest_id: Uuid,
    pub recipient_user_id: Uuid,
    pub email_to: String,
    pub subject: String,
    pub body_html: String,
    /// Per-mail custom headers (RFC 5322 name → string), carried through
    /// enqueue's `headers` parameter verbatim.
    pub headers: Value,
    /// The chatter host edge (model, res_id) mail records on the row.
    pub model: String,
    pub queued_at: DateTime<Utc>,
}

/// The seam. Implementations must make delivery-or-refusal atomic from the
/// caller's point of view: an `Err(DigestError::MailDelivery(_))` means
/// nothing was queued for this recipient.
#[async_trait]
pub trait DigestMailSeam: Send + Sync {
    async fn send_digest_mail(&self, mail: &OutgoingDigestMail) -> Result<Uuid, DigestError>;
}

/// The real seam over backbone-mail's public services.
pub struct DefaultMailSeam {
    pool: sqlx::PgPool,
    /// Optional fallback sender (the upstream email_from chain; the mail
    /// service defaults when absent).
    pub email_from: Option<String>,
}

impl DefaultMailSeam {
    pub fn new(pool: sqlx::PgPool) -> Self {
        Self { pool, email_from: None }
    }
}

#[async_trait]
impl DigestMailSeam for DefaultMailSeam {
    async fn send_digest_mail(&self, mail: &OutgoingDigestMail) -> Result<Uuid, DigestError> {
        use backbone_mail::application::service::message_write_service::MessagePostCommand;
        use backbone_mail::application::service::message_write_service::MessageWriteService;
        use backbone_mail::application::service::mail_queue_write_service::MailQueueWriteService;

        let messages = MessageWriteService::new(self.pool.clone());
        let posted = messages
            .message_post(MessagePostCommand {
                body: mail.body_html.clone(),
                subject: Some(mail.subject.clone()),
                message_type: "email".into(),
                subtype_id: None,
                subtype_name: None,
                is_internal: false,
                author_id: None,
                author_guest_id: None,
                email_from: self.email_from.clone(),
                reply_to: None,
                model: Some(mail.model.clone()),
                res_id: Some(mail.digest_id),
                record_name: Some(mail.subject.clone()),
                recipients: Vec::new(),
            })
            .await
            .map_err(|e| DigestError::MailDelivery(e.to_string()))?;

        let queue = MailQueueWriteService::new(self.pool.clone());
        queue
            .enqueue(
                posted.message_id,
                &mail.email_to,
                None,
                None,
                Some(&mail.headers),
                None,
                Some(&mail.model),
                Some(mail.digest_id),
            )
            .await
            .map_err(|e| DigestError::MailDelivery(e.to_string()))
    }
}

/// The test/instrumentation double: records everything, optionally fails.
#[derive(Debug, Default)]
pub struct RecordingMailSeam {
    pub sent: std::sync::Mutex<Vec<OutgoingDigestMail>>,
    pub fail_with: std::sync::Mutex<Option<String>>,
}

impl RecordingMailSeam {
    pub fn new() -> Self {
        Self::default()
    }

    /// Make the NEXT (and every subsequent) send fail with a
    /// mail-delivery-shaped error until cleared.
    pub fn fail_next(&self, reason: &str) {
        *self.fail_with.lock().unwrap_or_else(|e| e.into_inner()) = Some(reason.into());
    }

    pub fn clear_failure(&self) {
        *self.fail_with.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    pub fn sent(&self) -> Vec<OutgoingDigestMail> {
        self.sent.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

#[async_trait]
impl DigestMailSeam for RecordingMailSeam {
    async fn send_digest_mail(&self, mail: &OutgoingDigestMail) -> Result<Uuid, DigestError> {
        if let Some(reason) = self.fail_with.lock().unwrap_or_else(|e| e.into_inner()).clone() {
            return Err(DigestError::MailDelivery(reason));
        }
        let id = Uuid::new_v4();
        self.sent
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(mail.clone());
        Ok(id)
    }
}
