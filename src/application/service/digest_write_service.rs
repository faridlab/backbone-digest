//! The digest-row verbs (hand-written; user-owned): create, cadence
//! change, KPI enablement, subscription, and the state switch — the
//! write path the host's guarded routes and the growth loop drive.
//!
//! Every verb here is one of the schema's declared writers:
//! - `create_digest` fills `next_run_date` at create (R-DG13's create
//!   arm: today + the cadence delta, UTC);
//! - `set_periodicity` is the POST verb with the value whitelist
//!   (R-DG9 — upstream's mutating GET is banned by ADR-0019); changing
//!   cadence RESETS `next_run_date` from the new cadence (R-DG13's
//!   change arm);
//! - `enable_kpi` validates the key against the COMPOSED registry — an
//!   unknown key is the typed refusal `kpi_not_registered` (R-DG1),
//!   never silent data;
//! - `subscribe_user` is the subscribe verb the growth loop's handler
//!   drives (R-DG10) — idempotent by the (digest, user) unique key, so
//!   at-least-once bus delivery cannot double-subscribe;
//! - `set_state` is the 2-value machine's one-liner activate/deactivate
//!   (the cron's due scan filters on `activated`).

use chrono::{Datelike, NaiveDate, Utc};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::application::service::digest_error::DigestError;
use crate::application::service::kpi_registry::KpiRegistry;

/// The cadence enum (mirrors the `digest_periodicity` Postgres enum).
/// The ladder order IS the declaration order — see [`Self::next_rung`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DigestPeriodicity {
    Daily,
    Weekly,
    Monthly,
    Quarterly,
}

impl DigestPeriodicity {
    /// Parse the R-DG9 whitelist (the ONLY values accepted on the
    /// cadence-change verb). `None` = refused.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "daily" => Some(DigestPeriodicity::Daily),
            "weekly" => Some(DigestPeriodicity::Weekly),
            "monthly" => Some(DigestPeriodicity::Monthly),
            "quarterly" => Some(DigestPeriodicity::Quarterly),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            DigestPeriodicity::Daily => "daily",
            DigestPeriodicity::Weekly => "weekly",
            DigestPeriodicity::Monthly => "monthly",
            DigestPeriodicity::Quarterly => "quarterly",
        }
    }

    /// The next_run_date delta in CALENDAR units (R-DG13): 1 day / 1
    /// week / 1 month / 3 months from `from`. Calendar months, not 30-day
    /// blobs — upstream adds `relativedelta(months=1|3)`.
    pub fn advance(&self, from: NaiveDate) -> NaiveDate {
        match self {
            DigestPeriodicity::Daily => from + chrono::Duration::days(1),
            DigestPeriodicity::Weekly => from + chrono::Duration::weeks(1),
            DigestPeriodicity::Monthly => add_months(from, 1),
            DigestPeriodicity::Quarterly => add_months(from, 3),
        }
    }

    /// The ladder's next rung (R-DG3): daily→weekly→monthly→quarterly,
    /// monotonic. Quarterly is the FLOOR — there is no reverse rung and
    /// no fifth rung.
    pub fn next_rung(&self) -> Option<DigestPeriodicity> {
        match self {
            DigestPeriodicity::Daily => Some(DigestPeriodicity::Weekly),
            DigestPeriodicity::Weekly => Some(DigestPeriodicity::Monthly),
            DigestPeriodicity::Monthly => Some(DigestPeriodicity::Quarterly),
            DigestPeriodicity::Quarterly => None,
        }
    }
}

/// Calendar-month arithmetic (the `relativedelta` port): clamp the day
/// when the target month is shorter.
pub fn add_months(from: NaiveDate, months: i32) -> NaiveDate {
    let total = from.year() * 12 + (from.month0() as i32) + months;
    let year = total.div_euclid(12);
    let month0 = total.rem_euclid(12) as u32;
    let day = from.day().min(days_in_month(year, month0 + 1));
    NaiveDate::from_ymd_opt(year, month0 + 1, day).expect("valid ymd by construction")
}

fn days_in_month(year: i32, month: u32) -> u32 {
    let (y, m) = if month == 12 { (year + 1, 1) } else { (year, month + 1) };
    let first_of_next = NaiveDate::from_ymd_opt(y, m, 1).expect("valid first of month");
    (first_of_next - chrono::Duration::days(1)).day()
}

/// One digest config row as the engines read it.
#[derive(Debug, Clone)]
pub struct DigestRow {
    pub id: Uuid,
    pub name: String,
    pub periodicity: DigestPeriodicity,
    /// NULL = never mailed (R-DG13's nullable-with-meaning spine).
    pub next_run_date: Option<NaiveDate>,
    pub state: String,
    pub company_id: Uuid,
}

impl DigestRow {
    pub fn is_activated(&self) -> bool {
        self.state == "activated"
    }
}

/// The write surface.
pub struct DigestWriteService {
    pool: PgPool,
    registry: std::sync::Arc<KpiRegistry>,
}

impl DigestWriteService {
    pub fn new(pool: PgPool, registry: std::sync::Arc<KpiRegistry>) -> Self {
        Self { pool, registry }
    }

    /// Create a digest; `next_run_date` is filled at create from the
    /// cadence delta (R-DG13). State starts `activated`.
    #[allow(clippy::too_many_arguments)]
    pub async fn create_digest(
        &self,
        name: &str,
        company_id: Uuid,
        periodicity: DigestPeriodicity,
        today: NaiveDate,
    ) -> Result<Uuid, DigestError> {
        let id = Uuid::new_v4();
        sqlx::query(
            r#"INSERT INTO digest.digest_digests
                   (id, name, periodicity, next_run_date, state, company_id)
               VALUES ($1, $2, $3::digest_periodicity, $4, 'activated', $5)"#,
        )
        .bind(id)
        .bind(name)
        .bind(periodicity.as_str())
        .bind(periodicity.advance(today))
        .bind(company_id)
        .execute(&self.pool)
        .await?;
        Ok(id)
    }

    /// The R-DG9 cadence-change verb: POST-shape, whitelist-validated
    /// (refusal `periodicity_value_refused` on any other value), and
    /// next_run_date RESETS from the new cadence (R-DG13's change arm).
    /// Deliberately does NOT touch the ladder — the ladder degrades
    /// forward only, on the cron path only.
    pub async fn set_periodicity(
        &self,
        digest_id: Uuid,
        value: &str,
        today: NaiveDate,
    ) -> Result<(), DigestError> {
        let Some(p) = DigestPeriodicity::parse(value) else {
            return Err(DigestError::Invalid(format!(
                "periodicity '{value}' refused — the whitelist is daily|weekly|monthly|quarterly (error code: periodicity_value_refused)"
            )));
        };
        let n = sqlx::query(
            r#"UPDATE digest.digest_digests
               SET periodicity = $2::digest_periodicity,
                   next_run_date = $3
               WHERE id = $1"#,
        )
        .bind(digest_id)
        .bind(p.as_str())
        .bind(p.advance(today))
        .execute(&self.pool)
        .await?
        .rows_affected();
        if n == 0 {
            return Err(DigestError::NotFound(digest_id));
        }
        Ok(())
    }

    /// Enable one registry key on one digest. The key is validated
    /// against the COMPOSED registry — unknown keys are the typed
    /// refusal `kpi_not_registered` (R-DG1), never silent data.
    pub async fn enable_kpi(&self, digest_id: Uuid, kpi_key: &str) -> Result<(), DigestError> {
        if self.registry.get(kpi_key).is_none() {
            return Err(DigestError::Invalid(format!(
                "KPI '{kpi_key}' is not in the composed registry (error code: kpi_not_registered)"
            )));
        }
        sqlx::query(
            r#"INSERT INTO digest.digest_digest_kpis (digest_id, kpi_key)
               VALUES ($1, $2)
               ON CONFLICT (digest_id, kpi_key)
               DO NOTHING"#,
        )
        .bind(digest_id)
        .bind(kpi_key)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Disable one registry key on one digest (row-exists = enabled).
    pub async fn disable_kpi(&self, digest_id: Uuid, kpi_key: &str) -> Result<(), DigestError> {
        sqlx::query(
            r#"DELETE FROM digest.digest_digest_kpis
               WHERE digest_id = $1 AND kpi_key = $2"#,
        )
        .bind(digest_id)
        .bind(kpi_key)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// The subscribe verb (R-DG10's target; the host's self-service
    /// routes use it too). Idempotent by the (digest, user) unique key:
    /// an existing row — tombstoned or not — flips (back) to
    /// `subscribed`; `extra_metadata` (e.g. the growth loop's
    /// `auto_subscribed` marker) merges on conflict. Returns TRUE when
    /// this call created the row (the first-subscription edge the audit
    /// fact names).
    pub async fn subscribe_user(
        &self,
        digest_id: Uuid,
        user_id: Uuid,
        extra_metadata: Option<serde_json::Value>,
    ) -> Result<bool, DigestError> {
        let empty = serde_json::json!({});
        let meta = extra_metadata.unwrap_or(empty);
        let inserted = sqlx::query(
            r#"INSERT INTO digest.digest_subscriptions (digest_id, user_id, state, metadata)
               VALUES ($1, $2, 'subscribed', $3::jsonb)
               ON CONFLICT (digest_id, user_id)
               DO UPDATE SET state = 'subscribed',
                             unsubscribed_at = NULL,
                             metadata = (digest.digest_subscriptions.metadata - 'deleted_at')
                                        || excluded.metadata
               RETURNING (xmax = 0) AS inserted"#,
        )
        .bind(digest_id)
        .bind(user_id)
        .bind(meta)
        .fetch_one(&self.pool)
        .await?
        .try_get::<bool, _>("inserted")
        .unwrap_or(false);
        Ok(inserted)
    }

    /// The 2-value state machine's one-liners (DG-2): any→any, no
    /// guards. `activated` puts the digest back in the due scan.
    pub async fn set_state(&self, digest_id: Uuid, activated: bool) -> Result<(), DigestError> {
        let value = if activated { "activated" } else { "deactivated" };
        let n = sqlx::query(
            r#"UPDATE digest.digest_digests
               SET state = $2::digest_state
               WHERE id = $1"#,
        )
        .bind(digest_id)
        .bind(value)
        .execute(&self.pool)
        .await?
        .rows_affected();
        if n == 0 {
            return Err(DigestError::NotFound(digest_id));
        }
        Ok(())
    }

    /// Read one digest row.
    pub async fn get_digest(&self, digest_id: Uuid) -> Result<Option<DigestRow>, DigestError> {
        let row = sqlx::query_as::<_, (Uuid, String, String, Option<NaiveDate>, String, Uuid)>(
            r#"SELECT id, name, periodicity::text, next_run_date, state::text, company_id
               FROM digest.digest_digests WHERE id = $1"#,
        )
        .bind(digest_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|(id, name, p, next_run_date, state, company_id)| DigestRow {
            id,
            name,
            periodicity: DigestPeriodicity::parse(&p)
                .unwrap_or(DigestPeriodicity::Daily),
            next_run_date,
            state,
            company_id,
        }))
    }

    /// Stamp a metadata key on one subscription row (the growth loop's
    /// `first_digest_sent_at` marker rides this).
    pub async fn stamp_subscription_metadata(
        &self,
        digest_id: Uuid,
        user_id: Uuid,
        patch: serde_json::Value,
    ) -> Result<(), DigestError> {
        sqlx::query(
            r#"UPDATE digest.digest_subscriptions
               SET metadata = metadata || $3::jsonb
               WHERE digest_id = $1 AND user_id = $2"#,
        )
        .bind(digest_id)
        .bind(user_id)
        .bind(patch)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

/// Today's UTC date (the declination: no tz home — DG-5's ruling, the
/// windows share it).
pub fn utc_today(now: chrono::DateTime<Utc>) -> NaiveDate {
    now.date_naive()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whitelist_is_exact() {
        for ok in ["daily", "weekly", "monthly", "quarterly"] {
            assert!(DigestPeriodicity::parse(ok).is_some(), "{ok} must parse");
        }
        for refused in ["Daily", "biweekly", "", "yearly", "hourly"] {
            assert!(DigestPeriodicity::parse(refused).is_none(), "{refused} must be refused");
        }
    }

    #[test]
    fn ladder_order_is_monotonic_with_quarterly_floor() {
        assert_eq!(DigestPeriodicity::Daily.next_rung(), Some(DigestPeriodicity::Weekly));
        assert_eq!(DigestPeriodicity::Weekly.next_rung(), Some(DigestPeriodicity::Monthly));
        assert_eq!(DigestPeriodicity::Monthly.next_rung(), Some(DigestPeriodicity::Quarterly));
        assert_eq!(DigestPeriodicity::Quarterly.next_rung(), None);
    }

    #[test]
    fn advance_uses_calendar_units() {
        let d = NaiveDate::from_ymd_opt(2026, 1, 31).unwrap();
        // Monthly from Jan 31 clamps to Feb 28 (2026 not a leap year).
        assert_eq!(DigestPeriodicity::Monthly.advance(d), NaiveDate::from_ymd_opt(2026, 2, 28).unwrap());
        // Quarterly from Jan 31 → Apr 30.
        assert_eq!(DigestPeriodicity::Quarterly.advance(d), NaiveDate::from_ymd_opt(2026, 4, 30).unwrap());
        assert_eq!(DigestPeriodicity::Daily.advance(d), NaiveDate::from_ymd_opt(2026, 2, 1).unwrap());
        // Year rollover.
        let dec31 = NaiveDate::from_ymd_opt(2026, 12, 31).unwrap();
        assert_eq!(DigestPeriodicity::Monthly.advance(dec31), NaiveDate::from_ymd_opt(2027, 1, 31).unwrap());
    }
}
