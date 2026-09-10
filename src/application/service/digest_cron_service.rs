//! The daily pull sweep + the anti-spam slowdown ladder (hand-written;
//! user-owned) — engines 3 and 4.
//!
//! # The sweep (`digest::send_due`, schedule `41 2 * * *`, posture pull)
//!
//! The due-date column IS the queue (R-DG4): `next_run_date <= today AND
//! state = 'activated'`. NO self-arming trigger exists anywhere — nothing
//! in the digest lifecycle publishes an arming fact; the daily interval
//! is not a floor, it is the whole schedule.
//!
//! Per digest (one batch, one commit — commit_per_batch):
//!
//! 1. **claim** — `FOR UPDATE SKIP LOCKED` (pickup_lock): two sweeps
//!    must not double-send. The claim transaction is HELD through the
//!    digest's processing and released by its own commit, so a concurrent
//!    sweep skips the locked row;
//! 2. **slowdown check** (cron path only — a manual Send Now never
//!    degrades anything): if NO active recipient logged in within the
//!    cadence window, degrade one rung and audit
//!    `digest_slowdown_degraded`. The signal is sapiens
//!    `users.last_login` through the fail-closed recipient port — when
//!    the port cannot answer, the ladder HOLDS the cadence and reports
//!    `slowdown_signal_unavailable` (never degrades on a guess);
//! 3. **per recipient** — render under the recipient's own fence
//!    (condition 16) and enqueue through the mail seam (audit
//!    `digest_email_sent`), tip marker after enqueue;
//! 4. **advance** — `next_run_date` moves by the (possibly degraded)
//!    cadence — UNLESS a mail-delivery failure occurred (then
//!    deliberately unadvanced: the digest retries whole next day);
//! 5. **per-digest isolation** (the decided delta vs upstream's
//!    die-mid-loop): one digest's failure is logged, audited, and left
//!    due — the sweep CONTINUES to later digests.
//!
//! Scope note (operational): the sweep is a SYSTEM job that must see every
//! due digest regardless of who the rows belong to. The module ships no
//! tenancy axis (ADR-0029); in a composed deployment the tables are
//! org-fenced by the composing service's decorator, so the sweep runs on
//! the host's cron pool — a connection outside the request-scoped fence
//! (the table owner's, the migrations' role).
//!
//! # The ladder (R-DG3)
//!
//! daily → weekly → monthly → quarterly, one rung per due sweep with no
//! login in the window (daily 2d / weekly 7d / monthly 1m / quarterly
//! 3m). Monotonic: there is NO reverse rung — nothing ever speeds a
//! digest back up (only `set_periodicity` changes cadence by hand).
//! Quarterly is the floor.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::application::service::digest_error::DigestError;
use crate::application::service::digest_render_service::{active_recipients, DigestRenderService};
use crate::application::service::digest_write_service::{
    add_months, DigestPeriodicity, DigestRow,
};
use crate::application::service::engagement_port::{RecipientContextPort, RecipientContextSlot};

impl DigestPeriodicity {
    /// The ladder's login-recency window (R-DG3's table): daily 2 days,
    /// weekly 7, monthly 1 calendar month, quarterly 3.
    pub fn login_window_since(&self, now: DateTime<Utc>) -> DateTime<Utc> {
        match self {
            DigestPeriodicity::Daily => now - chrono::Duration::days(2),
            DigestPeriodicity::Weekly => now - chrono::Duration::days(7),
            DigestPeriodicity::Monthly => add_months(now.date_naive(), -1)
                .and_hms_opt(0, 0, 0)
                .expect("valid")
                .and_utc(),
            DigestPeriodicity::Quarterly => add_months(now.date_naive(), -3)
                .and_hms_opt(0, 0, 0)
                .expect("valid")
                .and_utc(),
        }
    }
}

/// One sweep run's tally (telemetry + probe surface).
#[derive(Debug, Default, Clone)]
pub struct SweepOutcome {
    /// Digests processed this run (claimed and completed the loop body).
    pub claimed: usize,
    /// Mails enqueued.
    pub sent_mails: usize,
    /// Digests the ladder degraded this run (cron path only).
    pub degraded: Vec<Uuid>,
    /// Digests left UNADVANCED by a mail-delivery failure (retry whole
    /// next day).
    pub mail_delivery_failures: Vec<Uuid>,
    /// Digests that hit an unexpected failure and were isolated (logged,
    /// audited, left due; the sweep continued).
    pub isolated_failures: Vec<Uuid>,
}

/// The sweep + manual-send engine.
pub struct DigestCronService {
    pool: PgPool,
    render: Arc<DigestRenderService>,
    port: RecipientContextSlot,
}

impl DigestCronService {
    pub fn new(pool: PgPool, render: Arc<DigestRenderService>, port: RecipientContextSlot) -> Self {
        Self { pool, render, port }
    }

    /// The daily pull (the `digest::send_due` handler body). Claims due
    /// digests one at a time (FOR UPDATE SKIP LOCKED, held through each
    /// digest's batch commit) until the due set this run is drained.
    pub async fn run_daily_pull(&self, now: DateTime<Utc>) -> Result<SweepOutcome, DigestError> {
        let today = now.date_naive();
        let mut outcome = SweepOutcome::default();
        let mut processed: Vec<Uuid> = Vec::new();

        loop {
            let mut tx = self.pool.begin().await?;
            let claimed = sqlx::query_as::<_, (Uuid, String, String, Option<chrono::NaiveDate>, String)>(
                r#"SELECT id, name, periodicity::text, next_run_date, state::text
                   FROM digest.digest_digests
                   WHERE state = 'activated'
                     AND next_run_date IS NOT NULL
                     AND next_run_date <= $1
                     AND NOT (id = ANY($2))
                   ORDER BY next_run_date, id
                   FOR UPDATE SKIP LOCKED
                   LIMIT 1"#,
            )
            .bind(today)
            .bind(&processed)
            .fetch_optional(&mut *tx)
            .await?;

            let Some((id, name, p, next_run_date, state)) = claimed else {
                // Due set drained (or everything else is locked by a
                // concurrent sweep).
                break;
            };
            let digest = DigestRow {
                id,
                name,
                periodicity: DigestPeriodicity::parse(&p).unwrap_or(DigestPeriodicity::Daily),
                next_run_date,
                state,
            };
            processed.push(digest.id);
            outcome.claimed += 1;

            // ---- per-digest batch (isolated; one commit) ----------------
            match self.process_digest(&mut tx, &digest, now, &mut outcome).await {
                Ok(()) => { tx.commit().await?; }
                Err(e) => {
                    // The batch's own SQL failed inside the claim tx:
                    // roll back, record, CONTINUE (the decided delta).
                    tx.rollback().await?;
                    tracing::error!(
                        target: "digest::audit",
                        event = "digest_send_isolated",
                        digest_id = %digest.id,
                        error = %e,
                        "digest send failed; digest left due (no advance), sweep continues"
                    );
                    outcome.isolated_failures.push(digest.id);
                }
            }
        }
        Ok(outcome)
    }

    /// One digest's batch body inside the claim transaction. NEVER
    /// returns Err for recipient-level trouble (those are tallied into
    /// `outcome`); Err means the batch itself failed and must roll back.
    async fn process_digest(
        &self,
        tx: &mut sqlx::PgTransaction<'_>,
        digest: &DigestRow,
        now: DateTime<Utc>,
        outcome: &mut SweepOutcome,
    ) -> Result<(), DigestError> {
        let recipients = active_recipients(&self.pool, digest.id).await?;

        // ---- step 2: the slowdown check (cron path only) --------------
        let mut cadence = digest.periodicity;
        let user_ids: Vec<Uuid> = recipients.iter().map(|r| r.user_id).collect();
        let since = digest.periodicity.login_window_since(now);
        match self.port.any_logged_in_since(&user_ids, since).await {
            Ok(false) => {
                if let Some(next) = cadence.next_rung() {
                    sqlx::query(
                        "UPDATE digest.digest_digests SET periodicity = $2::digest_periodicity WHERE id = $1",
                    )
                    .bind(digest.id)
                    .bind(next.as_str())
                    .execute(&mut **tx)
                    .await?;
                    tracing::info!(
                        target: "digest::audit",
                        event = "digest_slowdown_degraded",
                        digest_id = %digest.id,
                        from = cadence.as_str(),
                        to = next.as_str(),
                        recipients = user_ids.len()
                    );
                    cadence = next;
                    outcome.degraded.push(digest.id);
                }
                // Quarterly floor: no fifth rung — stays.
            }
            Ok(true) => { /* engaged: no degradation */ }
            Err(e) => {
                // Fail-closed signal: HOLD the cadence, never degrade on
                // a guess. The send still proceeds (degradation is the
                // only decision the signal feeds).
                tracing::error!(
                    target: "digest::audit",
                    event = "slowdown_signal_unavailable",
                    digest_id = %digest.id,
                    error = %e,
                    "slowdown signal unavailable through the recipient port; cadence held"
                );
            }
        }

        // ---- step 3: per-recipient render + enqueue -------------------
        let mut mail_failure = false;
        let mut digest_failure: Option<String> = None;
        for recipient in &recipients {
            match self.render.send_to_recipient(digest, recipient, now).await {
                Ok((_plan, _mail_id)) => {
                    outcome.sent_mails += 1;
                }
                Err(DigestError::MailDelivery(why)) => {
                    // The MailDeliveryException port: WARNING only; the
                    // digest stays due and retries whole next day.
                    tracing::warn!(
                        digest_id = %digest.id,
                        user_id = %recipient.user_id,
                        reason = %why,
                        "mail delivery refused for recipient; digest will retry whole next day (no advance)"
                    );
                    mail_failure = true;
                }
                Err(e) => {
                    // Unexpected per-recipient failure: log, isolate,
                    // continue with the remaining recipients.
                    tracing::error!(
                        digest_id = %digest.id,
                        user_id = %recipient.user_id,
                        error = %e,
                        "unexpected recipient send failure (isolated)"
                    );
                    digest_failure = Some(e.to_string());
                }
            }
        }

        // ---- step 4: the advance decision -----------------------------
        if mail_failure || digest_failure.is_some() {
            // Deliberately UNADVANCED: the digest remains due and retries
            // whole next day (at-least-once is the family's mail trade).
            if mail_failure {
                outcome.mail_delivery_failures.push(digest.id);
            } else {
                outcome.isolated_failures.push(digest.id);
            }
            return Ok(());
        }
        let advanced_to = cadence.advance(now.date_naive());
        let n = sqlx::query("UPDATE digest.digest_digests SET next_run_date = $2 WHERE id = $1")
            .bind(digest.id)
            .bind(advanced_to)
            .execute(&mut **tx)
            .await?
            .rows_affected();
        if n == 0 {
            return Err(DigestError::SendFailed(format!(
                "digest {} vanished mid-batch (advance matched 0 rows)",
                digest.id
            )));
        }
        Ok(())
    }

    /// Manual "Send Now": render + enqueue to every active recipient
    /// NOW. NEVER degrades anything and NEVER advances next_run_date
    /// (an extra send outside the schedule — the ladder is cron-only by
    /// R-DG3). Returns (sent, mail-delivery failures, unexpected
    /// failures).
    pub async fn send_now(
        &self,
        digest_id: Uuid,
        now: DateTime<Utc>,
    ) -> Result<(usize, usize, usize), DigestError> {
        let Some(digest) = (|| async {
            let row = sqlx::query_as::<_, (Uuid, String, String, Option<chrono::NaiveDate>, String)>(
                r#"SELECT id, name, periodicity::text, next_run_date, state::text
                   FROM digest.digest_digests WHERE id = $1"#,
            )
            .bind(digest_id)
            .fetch_optional(&self.pool)
            .await?;
            Ok::<_, DigestError>(row.map(|(id, name, p, next_run_date, state)| DigestRow {
                id,
                name,
                periodicity: DigestPeriodicity::parse(&p).unwrap_or(DigestPeriodicity::Daily),
                next_run_date,
                state,
            }))
        })()
        .await?
        else {
            return Err(DigestError::NotFound(digest_id));
        };

        let recipients = active_recipients(&self.pool, digest.id).await?;
        let (mut sent, mut mail_fail, mut unexpected) = (0, 0, 0);
        for recipient in &recipients {
            match self.render.send_to_recipient(&digest, recipient, now).await {
                Ok(_) => sent += 1,
                Err(DigestError::MailDelivery(why)) => {
                    tracing::warn!(digest_id = %digest.id, reason = %why, "manual send: mail delivery refused");
                    mail_fail += 1;
                }
                Err(e) => {
                    tracing::error!(digest_id = %digest.id, error = %e, "manual send: unexpected failure");
                    unexpected += 1;
                }
            }
        }
        Ok((sent, mail_fail, unexpected))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ladder_windows_follow_the_rule_table() {
        let now = DateTime::parse_from_rfc3339("2026-08-31T02:41:00Z").unwrap().with_timezone(&Utc);
        let d = DigestPeriodicity::Daily.login_window_since(now);
        assert_eq!((now - d).num_days(), 2);
        let w = DigestPeriodicity::Weekly.login_window_since(now);
        assert_eq!((now - w).num_days(), 7);
        // Calendar months: Aug 31 minus one month clamps to Jul 31.
        let m = DigestPeriodicity::Monthly.login_window_since(now);
        assert_eq!(m.date_naive(), chrono::NaiveDate::from_ymd_opt(2026, 7, 31).unwrap());
        let q = DigestPeriodicity::Quarterly.login_window_since(now);
        assert_eq!(q.date_naive(), chrono::NaiveDate::from_ymd_opt(2026, 5, 31).unwrap());
    }
}
