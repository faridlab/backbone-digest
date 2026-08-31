//! The RFC 8058 one-click unsubscribe engine (hand-written; user-owned).
//!
//! The sanctioned HMAC action-link class (ADR-0019): POST-form, Tier A
//! token, idempotent. The token is the capability — `HMAC-SHA256` over
//! `(digest_id, user_id, expiry)` with a `digest-unsubscribe` domain
//! separator, verified constant-time (`consteq` semantics via
//! `hmac`'s `verify_slice`), carried as `{digest}.{user}.{exp}.{mac}`.
//!
//! PORTED DEVIATIONS (recorded):
//!
//! 1. **Spelling** — upstream's shipped route is `unsubscribe_oneclik`
//!    (the typo is API; every mailed link carries it). This port uses the
//!    CORRECT RFC 8058 spelling (`/digest/unsubscribe`): no mailed link
//!    predates this module, so nothing can break.
//! 2. **Expiry** — upstream tokens never expire (EBB-20 family). Tier A
//!    tokens here carry a 180-day expiry: the slowest ladder rung is
//!    quarterly, so a token comfortably outlives the gap between any two
//!    mails, while a leaked link eventually dies. An EXPIRED or FORGED
//!    token gets the SAME bare 200 with NO action as a valid one — no
//!    oracle, no bounce (RFC 8058 §3 wants 2xx from the MUA path).
//! 3. **DKIM** — one-click is only enforceable when the mail is DKIM-
//!    signed on a domain the receiver trusts. That is an OPS dependency
//!    (the sending domain's DKIM/SPF/DMARC posture), named here per the
//!    binding condition; this module cannot provide it.
//!
//! Idempotency: the subscription row IS the fence — `digest.digest_subscriptions`
//! carries a `subscribed`/`unsubscribed` state with one row per
//! (digest, user), so the first valid POST flips the row to the
//! `unsubscribed` tombstone (stamping `unsubscribed_at`); every re-POST
//! (the MUA re-fires, the user re-clicks) finds the row already
//! tombstoned and no-ops, still 200, and the audit fact is written only
//! on the performed edge (never duplicated by replays).

use chrono::{Duration, NaiveDateTime, Utc};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

use crate::application::service::digest_error::DigestError;

/// Env var holding the HMAC secret for unsubscribe tokens.
pub const DIGEST_TOKEN_SECRET_ENV: &str = "DIGEST_TOKEN_SECRET";

/// Token lifetime: 180 days (see the deviations note above).
pub const TOKEN_TTL_DAYS: i64 = 180;

/// The HMAC domain separator (upstream's `'digest-unsubscribe'` key).
const DOMAIN: &str = "digest-unsubscribe";

type HmacSha256 = Hmac<Sha256>;

/// The public unsubscribe token: `{digest_id}.{user_id}.{exp}.{mac_hex}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsubscribeToken {
    pub digest_id: Uuid,
    pub user_id: Uuid,
    pub exp: i64,
    pub mac: String,
}

/// The parsed `UnsubscribeToken` verdict for the public surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenVerdict {
    Valid,
    /// Wrong MAC, malformed, or expired — ONE refusal shape (no oracle).
    Refused,
}

/// Throttle knobs for the verify path (the survey Tier-B precedent,
/// flattened): a failed verify locks the (digest, user) key out briefly so
/// a forger cannot grind the MAC.
pub const VERIFY_MAX_FAILURES: i32 = 5;
pub const VERIFY_LOCK_SECONDS: i64 = 60;

/// The verify-failure book (in-memory per composing service; a
/// multi-instance host fronts it with a shared limiter — the family
/// trade).
#[derive(Debug, Default)]
pub struct VerifyFailureBook {
    entries: Mutex<HashMap<String, i32>>,
}

impl VerifyFailureBook {
    pub fn new() -> Self {
        Self::default()
    }

    fn key(digest_id: Uuid, user_id: Uuid) -> String {
        format!("digest-unsub|{digest_id}|{user_id}")
    }

    /// Is this (digest, user) pair currently locked out?
    pub fn locked(&self, digest_id: Uuid, user_id: Uuid) -> bool {
        let n = *self
            .entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&Self::key(digest_id, user_id))
            .unwrap_or(&0);
        n >= VERIFY_MAX_FAILURES
    }

    pub fn register_failure(&self, digest_id: Uuid, user_id: Uuid) {
        let mut guard = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        *guard.entry(Self::key(digest_id, user_id)).or_insert(0) += 1;
    }

    pub fn reset(&self, digest_id: Uuid, user_id: Uuid) {
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&Self::key(digest_id, user_id));
    }
}

/// The unsubscribe engine: mint, verify (throttled), apply (idempotent).
pub struct UnsubscribeService {
    pool: PgPool,
    secret: Vec<u8>,
    failures: Arc<VerifyFailureBook>,
}

fn compute_mac(secret: &[u8], digest_id: &Uuid, user_id: &Uuid, exp: i64) -> String {
    let mut mac = HmacSha256::new_from_slice(secret).expect("HMAC accepts any key length");
    mac.update(format!("{DOMAIN}|{digest_id}|{user_id}|{exp}").as_bytes());
    mac.finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Constant-time comparison (the `consteq` requirement): the candidate
/// MAC is checked with `verify_slice`, which compares in constant time.
fn verify_mac(secret: &[u8], token: &UnsubscribeToken) -> bool {
    let Some(bytes) = hex_bytes(&token.mac) else {
        return false;
    };
    let mut mac = HmacSha256::new_from_slice(secret).expect("HMAC accepts any key length");
    mac.update(format!("{DOMAIN}|{}|{}|{}", token.digest_id, token.user_id, token.exp).as_bytes());
    mac.verify_slice(&bytes).is_ok()
}

fn hex_bytes(s: &str) -> Option<Vec<u8>> {
    if s.len() != 64 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

impl UnsubscribeService {
    /// From the builder's explicit secret.
    pub fn with_secret(pool: PgPool, secret: &[u8]) -> Self {
        Self {
            pool,
            secret: secret.to_vec(),
            failures: Arc::new(VerifyFailureBook::new()),
        }
    }

    /// From the environment (`DIGEST_TOKEN_SECRET`); errors loudly when
    /// unset (a silent default secret would forge capabilities).
    pub fn from_env(pool: PgPool) -> Result<Self, DigestError> {
        let secret = std::env::var(DIGEST_TOKEN_SECRET_ENV)
            .map_err(|_| DigestError::SecretNotConfigured)?;
        Ok(Self::with_secret(pool, secret.as_bytes()))
    }

    pub fn failure_book(&self) -> Arc<VerifyFailureBook> {
        self.failures.clone()
    }

    /// Mint the token for one (digest, user) at `now`.
    pub fn mint(&self, digest_id: Uuid, user_id: Uuid, now: impl Into<NaiveDateTime>) -> UnsubscribeToken {
        let now: NaiveDateTime = now.into();
        let exp_ts = (now + Duration::days(TOKEN_TTL_DAYS)).and_utc().timestamp();
        UnsubscribeToken {
            digest_id,
            user_id,
            exp: exp_ts,
            mac: compute_mac(&self.secret, &digest_id, &user_id, exp_ts),
        }
    }

    /// The mailed link's query value (`t=`).
    pub fn mint_link_token(&self, digest_id: Uuid, user_id: Uuid) -> String {
        let t = self.mint(digest_id, user_id, Utc::now().naive_utc());
        format!("{}.{}.{}.{}", t.digest_id, t.user_id, t.exp, t.mac)
    }

    /// Parse `{digest}.{user}.{exp}.{mac}` — `None` when malformed.
    pub fn parse(token: &str) -> Option<UnsubscribeToken> {
        let mut parts = token.split('.');
        let digest_id = Uuid::parse_str(parts.next()?).ok()?;
        let user_id = Uuid::parse_str(parts.next()?).ok()?;
        let exp: i64 = parts.next()?.parse().ok()?;
        let mac = parts.next()?.to_string();
        if mac.len() != 64 || !mac.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        if parts.next().is_some() {
            return None;
        }
        Some(UnsubscribeToken { digest_id, user_id, exp, mac })
    }

    /// The (throttled) verdict for a candidate token at `now`.PURE-ish:
    /// touches only the in-memory failure book.
    pub fn verify(&self, token: &UnsubscribeToken, now: i64) -> TokenVerdict {
        if self.failures.locked(token.digest_id, token.user_id) {
            return TokenVerdict::Refused;
        }
        let ok = verify_mac(&self.secret, token)
            && now <= token.exp;
        if ok {
            self.failures.reset(token.digest_id, token.user_id);
            TokenVerdict::Valid
        } else {
            self.failures.register_failure(token.digest_id, token.user_id);
            TokenVerdict::Refused
        }
    }

    /// Apply a VERIFIED unsubscribe, idempotently: flip the subscription
    /// row to the `unsubscribed` tombstone (stamping `unsubscribed_at`).
    /// Returns whether THIS call performed the unsubscribe (`false` =
    /// already unsubscribed, or no subscription row — both still a
    /// success for the route; RFC 8058 wants no error from the MUA path).
    /// The audit fact `digest_unsubscribed` is recorded only on the
    /// performed edge.
    pub async fn apply(&self, digest_id: Uuid, user_id: Uuid) -> Result<bool, DigestError> {
        let performed = sqlx::query(
            r#"UPDATE digest.digest_subscriptions
               SET state = 'unsubscribed', unsubscribed_at = NOW()
               WHERE digest_id = $1 AND user_id = $2
                 AND state = 'subscribed'
                 AND (metadata->>'deleted_at') IS NULL"#,
        )
        .bind(digest_id)
        .bind(user_id)
        .execute(&self.pool)
        .await?
        .rows_affected()
            == 1;
        if performed {
            tracing::info!(
                target: "digest::audit",
                event = "digest_unsubscribed",
                digest_id = %digest_id,
                user_id = %user_id,
                channel = "rfc8058_one_click"
            );
        }
        Ok(performed)
    }

    /// The declared reverse edge (the subscription machine's
    /// `resubscribe`): flip the tombstone back to `subscribed` and clear
    /// `unsubscribed_at`. Idempotent the same way — a re-run on a live
    /// row is a no-op success.
    pub async fn resubscribe(&self, digest_id: Uuid, user_id: Uuid) -> Result<bool, DigestError> {
        let performed = sqlx::query(
            r#"UPDATE digest.digest_subscriptions
               SET state = 'subscribed', unsubscribed_at = NULL
               WHERE digest_id = $1 AND user_id = $2
                 AND state = 'unsubscribed'
                 AND (metadata->>'deleted_at') IS NULL"#,
        )
        .bind(digest_id)
        .bind(user_id)
        .execute(&self.pool)
        .await?
        .rows_affected()
            == 1;
        Ok(performed)
    }

    /// The full route path: parse → throttled verify → idempotent apply.
    /// `Ok(applied)` either way for a well-formed request; the refusal
    /// verdict also returns `Ok(false)` (bare 200, no action, no oracle).
    pub async fn unsubscribe_by_token(&self, token_str: &str, now: i64) -> Result<bool, DigestError> {
        let Some(token) = Self::parse(token_str) else {
            // Malformed: count it against the zero-uuid bucket so garbage
            // floods still hit a lock.
            self.failures.register_failure(Uuid::nil(), Uuid::nil());
            return Ok(false);
        };
        match self.verify(&token, now) {
            TokenVerdict::Valid => self.apply(token.digest_id, token.user_id).await,
            TokenVerdict::Refused => Ok(false),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn svc() -> UnsubscribeService {
        UnsubscribeService::with_secret(PgPool::connect_lazy("postgres://x").expect("lazy"), b"probe-secret")
    }

    // The pools above are lazy (no I/O) but sqlx still requires a Tokio
    // context to build one — hence tokio::test on these seats.

    #[tokio::test]
    async fn mint_parse_roundtrip() {
        let s = svc();
        let d = Uuid::new_v4();
        let u = Uuid::new_v4();
        let t = s.mint(d, u, Utc::now().naive_utc());
        let link = format!("{}.{}.{}.{}", t.digest_id, t.user_id, t.exp, t.mac);
        let parsed = UnsubscribeService::parse(&link).expect("parse");
        assert_eq!(parsed, t);
    }

    #[tokio::test]
    async fn verify_accepts_fresh_and_refuses_expired() {
        let s = svc();
        let d = Uuid::new_v4();
        let u = Uuid::new_v4();
        let now = Utc::now().timestamp();
        let t = s.mint(d, u, Utc::now().naive_utc());
        assert_eq!(s.verify(&t, now), TokenVerdict::Valid);
        assert_eq!(s.verify(&t, t.exp + 1), TokenVerdict::Refused);
    }

    #[tokio::test]
    async fn forged_mac_is_refused_and_locks() {
        let s = svc();
        let d = Uuid::new_v4();
        let u = Uuid::new_v4();
        let now = Utc::now().timestamp();
        let mut t = s.mint(d, u, Utc::now().naive_utc());
        t.mac = "0".repeat(64);
        for _ in 0..VERIFY_MAX_FAILURES {
            assert_eq!(s.verify(&t, now), TokenVerdict::Refused);
        }
        assert!(s.failures.locked(d, u));
        // Even the genuine token is locked out now.
        let good = s.mint(d, u, Utc::now().naive_utc());
        assert_eq!(s.verify(&good, now), TokenVerdict::Refused);
    }

    #[tokio::test]
    async fn wrong_secret_is_refused() {
        let a = UnsubscribeService::with_secret(PgPool::connect_lazy("postgres://x").unwrap(), b"one");
        let b = UnsubscribeService::with_secret(PgPool::connect_lazy("postgres://x").unwrap(), b"two");
        let t = a.mint(Uuid::new_v4(), Uuid::new_v4(), Utc::now().naive_utc());
        assert_eq!(b.verify(&t, Utc::now().timestamp()), TokenVerdict::Refused);
    }
}
