//! The two base KPI compute functions (hand-written; user-owned).
//!
//! Ported sources (the spec's base field pair):
//!
//! - `kpi_res_users_connected` — Connected Users: upstream counts
//!   `res.users` with `login_date` inside the window. Here: sapiens
//!   `users.last_login` scoped to the RECIPIENT's company through an
//!   active `organization_users` membership — read through the
//!   fail-closed recipient port ([`super::engagement_port`]), never via
//!   a direct sapiens Cargo edge. An unwired port surfaces as
//!   `KpiError::Unavailable` (the KPI drops from that render with a
//!   warning — the fail-closed trade). Fence declaration:
//!   `CompanyData`.
//! - `kpi_mail_message_total` — Messages Sent: upstream search-counts
//!   `mail.message` rows of subtype comment with type in
//!   (comment, email, email_outgoing) inside the window — NOT
//!   company-filtered upstream (mail.message carries no company). Here:
//!   `messaging.mail_messages` with `message_type IN ('email','comment')`
//!   (the backbone enum's two comment-family values; 'notification' is the
//!   system family and 'sms' rides its own channel). Fence declaration:
//!   `SharedData`.
//!
//! Both computers scope their reads ONLY by the context's window and
//! `company_id` — they never consult the registry or any row-count-based
//! drop heuristic (that decision lives in
//! [`super::kpi_registry::KpiFenceDeclaration`]).

use chrono::{DateTime, Utc};
use sqlx::PgPool;

use crate::application::service::engagement_port::{RecipientContextPort, RecipientContextSlot};
use crate::application::service::kpi_registry::{
    KpiComputer, KpiError, KpiQueryContext, KpiValue, KPI_CONNECTED_USERS, KPI_MESSAGES_SENT,
};

/// Connected Users — company-scoped logins inside the window, resolved
/// through the recipient port (the module's only sapiens window).
pub struct ConnectedUsersKpi {
    port: RecipientContextSlot,
}

impl ConnectedUsersKpi {
    pub fn new(port: RecipientContextSlot) -> Self {
        Self { port }
    }
}

#[async_trait::async_trait]
impl KpiComputer for ConnectedUsersKpi {
    async fn compute(&self, _pool: &PgPool, ctx: &KpiQueryContext) -> Result<KpiValue, KpiError> {
        let n = self
            .port
            .count_connected(ctx.company_id, ctx.window_start, ctx.window_end)
            .await
            .map_err(|e| KpiError::Unavailable(e.to_string()))?;
        Ok(KpiValue::count(n))
    }
}

/// Messages Sent — shared (unfenced) message volume inside the window.
pub struct MessagesSentKpi;

const MESSAGES_SENT_SQL: &str = r#"
    SELECT count(*) AS n
    FROM messaging.mail_messages
    WHERE message_type IN ('email', 'comment')
      AND date >= $1
      AND date <  $2
      AND (metadata->>'deleted_at') IS NULL
"#;

#[async_trait::async_trait]
impl KpiComputer for MessagesSentKpi {
    async fn compute(&self, pool: &PgPool, ctx: &KpiQueryContext) -> Result<KpiValue, KpiError> {
        let n: i64 = sqlx::query_scalar(MESSAGES_SENT_SQL)
            .bind(ctx.window_start)
            .bind(ctx.window_end)
            .fetch_one(pool)
            .await
            .map_err(KpiError::Compute)?;
        Ok(KpiValue::count(n))
    }
}

/// The window pair (current, previous) for one of the three render
/// columns. Pure — probes assert the boundaries directly.
///
/// Upstream localizes `now` to the company calendar's timezone (the whole
/// reason `resource` is a dependency). DECLINATION (recorded): no timezone
/// home exists on the Company/calendar surface in-tree, so the port fixes
/// windows in UTC. A tz-aware port re-opens here — the register row tracks
/// it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeWindow {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

impl TimeWindow {
    /// The current period ending `now`, spanning `days`.
    pub fn current(now: DateTime<Utc>, days: i64) -> Self {
        Self { start: now - chrono::Duration::days(days), end: now }
    }

    /// The equally-sized period immediately before this one (the margin
    /// basis).
    pub fn previous(&self) -> Self {
        let span = self.end - self.start;
        Self { start: self.start - span, end: self.start }
    }
}

/// The three render columns, in render order (24h / 7d / 30d).
pub const WINDOW_SPANS_DAYS: [i64; 3] = [1, 7, 30];

pub fn windows_at(now: DateTime<Utc>) -> [(TimeWindow, TimeWindow); 3] {
    WINDOW_SPANS_DAYS
        .map(|days| TimeWindow::current(now, days))
        .map(|cur| (cur, cur.previous()))
}

/// The previous-period margin: `Some(pct)` only when the value CHANGED and
/// BOTH sides are non-zero — flat `None` otherwise (upstream's rule,
/// ported verbatim; `float_round(…, 2)` becomes the 2-decimal round here).
pub fn margin(current: f64, previous: f64) -> Option<f64> {
    if current != previous && current != 0.0 && previous != 0.0 {
        Some(((current - previous) / previous * 100.0 * 100.0).round() / 100.0)
    } else {
        None
    }
}

/// Register the base pair into a registry (the builder calls this at
/// `build()`; probes call it on fresh registries). The connected-users
/// computer is built over the recipient port slot, so it follows whatever
/// the host wires.
pub fn register_base_kpis(
    registry: &crate::application::service::kpi_registry::KpiRegistry,
    port: RecipientContextSlot,
) {
    use crate::application::service::kpi_registry::{KpiDefinition, KpiFenceDeclaration};
    let _ = registry.register(KpiDefinition {
        name: KPI_CONNECTED_USERS.into(),
        label: "Connected Users".into(),
        fence: KpiFenceDeclaration::CompanyData,
        computer: std::sync::Arc::new(ConnectedUsersKpi::new(port)),
    });
    let _ = registry.register(KpiDefinition {
        name: KPI_MESSAGES_SENT.into(),
        label: "Messages Sent".into(),
        fence: KpiFenceDeclaration::SharedData,
        computer: std::sync::Arc::new(MessagesSentKpi),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_are_utc_and_previous_shifts_exactly() {
        let now = Utc::now();
        let [(c1, p1), (c7, p7), (c30, p30)] = windows_at(now);
        assert_eq!(c1.end, now);
        assert_eq!((c1.end - c1.start).num_hours(), 24);
        assert_eq!(p1.end, c1.start);
        assert_eq!((p1.end - p1.start).num_hours(), 24);
        assert_eq!((c7.end - c7.start).num_days(), 7);
        assert_eq!(p7.end, c7.start);
        assert_eq!((c30.end - c30.start).num_days(), 30);
        assert_eq!(p30.end, c30.start);
    }

    #[test]
    fn margin_only_when_changed_and_both_nonzero() {
        assert_eq!(margin(10.0, 5.0), Some(100.0));
        assert_eq!(margin(5.0, 10.0), Some(-50.0));
        // Genuine zero: flat (no margin), never an error, never a drop.
        assert_eq!(margin(0.0, 5.0), None);
        assert_eq!(margin(5.0, 0.0), None);
        assert_eq!(margin(0.0, 0.0), None);
        // Unchanged: flat.
        assert_eq!(margin(5.0, 5.0), None);
        // Two-decimal rounding.
        assert_eq!(margin(3.0, 9.0), Some(-66.67));
    }
}
