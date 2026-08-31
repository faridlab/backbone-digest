//! The module's typed error surface (hand-written; user-owned).

use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum DigestError {
    #[error("db: {0}")]
    Db(#[from] sqlx::Error),

    /// The mail seam refused/failed the enqueue — the port of Odoo's
    /// `MailDeliveryException` class. The cron treats it as WARNING ONLY
    /// and does NOT advance `next_run_date` (the digest retries whole next
    /// day).
    #[error("mail delivery: {0}")]
    MailDelivery(String),

    /// Anything unexpected inside ONE digest's send. DECIDED DELTA vs
    /// upstream (recorded): upstream lets a non-mail exception kill the
    /// cron run mid-loop (later digests that day wait a full day). This
    /// port isolates per digest — the failure is recorded loudly, the
    /// digest is left due (no advance), and the loop CONTINUES.
    #[error("digest send failed: {0}")]
    SendFailed(String),

    #[error("digest {0} not found")]
    NotFound(Uuid),

    #[error("invalid: {0}")]
    Invalid(String),

    /// The unsubscribe token verdict. Deliberately COARSE on the wire: the
    /// public route never distinguishes expired from forged from malformed
    /// (no oracle) — one shape, one status.
    #[error("unsubscribe refused: {0}")]
    UnsubscribeRefused(String),

    /// The HMAC secret is not configured (builder `with_token_secret` or
    /// `DIGEST_TOKEN_SECRET`).
    #[error("token secret not configured")]
    SecretNotConfigured,

    /// The public base URL for mailed unsubscribe links is empty. Fails
    /// LOUD per digest — symmetric with [`DigestError::SecretNotConfigured`]:
    /// a digest mail whose one-click link would point at a guessable
    /// default host (e.g. a localhost fallback flowing into a production
    /// overlay) must not send. The base overlay's dev default never
    /// trips this; a production overlay resolves the env var to empty
    /// when it is undeclared.
    #[error("public base url not configured")]
    PublicBaseUrlNotConfigured,
}

impl DigestError {
    /// HTTP status for the route mapping (the survey refusal mapping
    /// shape). The unsubscribe refusal keeps a bare 200 at the ROUTE level
    /// (RFC 8058: no bounce, no oracle) — this mapping is for the guarded
    /// verbs.
    pub fn http_status(&self) -> u16 {
        match self {
            DigestError::NotFound(_) => 404,
            DigestError::Invalid(_) => 400,
            DigestError::UnsubscribeRefused(_) => 403,
            _ => 500,
        }
    }

    pub fn code(&self) -> &'static str {
        match self {
            DigestError::Db(_)
            | DigestError::SecretNotConfigured
            | DigestError::PublicBaseUrlNotConfigured => "internal_error",
            DigestError::MailDelivery(_) => "digest_mail_delivery",
            DigestError::SendFailed(_) => "digest_send_failed",
            DigestError::NotFound(_) => "digest_not_found",
            DigestError::Invalid(_) => "digest_invalid",
            DigestError::UnsubscribeRefused(_) => "digest_unsubscribe_refused",
        }
    }
}
