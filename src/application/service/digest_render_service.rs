//! The per-recipient render engine (hand-written; user-owned) — where
//! condition 16 lives.
//!
//! One digest renders PER RECIPIENT under the RECIPIENT's own fence:
//!
//! 1. the enabled keys load (rows referencing the registry's `kpi_<name>`
//!    namespace);
//! 2. for each key, the drop decision runs FIRST —
//!    [`KpiFenceDeclaration::renders_for`] on the registry entry's OWN
//!    fence declaration, never on any row count. An out-of-fence KPI is
//!    ABSENT from the mail (silently, by design) and its compute is
//!    never invoked; an in-fence KPI whose data is genuinely zero
//!    RENDERS `0`. Under RLS the two are row-count-identical — only the
//!    declaration separates them;
//! 3. each surviving KPI computes over the three render windows
//!    (24h / 7d / 30d, UTC — the declination) with the previous-period
//!    margin shown only when the value changed AND both sides are
//!    non-zero;
//! 4. the carousel tip picks (lowest-sequence unconsumed, group-gated)
//!    and is html-sanitized HERE, at render — never at rest;
//! 5. the RFC 8058 footer + headers mint a fresh Tier A token per mail
//!    (expiry is cheap when links regenerate every send).
//!
//! Delivery order (R-DGT1): ENQUEUE first, tip consumption marker AFTER
//! — [`DigestRenderService::deliver`] is the only place that writes the
//! marker, and only on a successful enqueue.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use crate::application::service::base_kpis::{margin, windows_at, TimeWindow};
use crate::application::service::digest_error::DigestError;
use crate::application::service::digest_mail_seam::{DigestMailSeam, OutgoingDigestMail};
use crate::application::service::digest_write_service::DigestRow;
use crate::application::service::engagement_port::{RecipientContextPort, RecipientContextSlot};
use crate::application::service::kpi_registry::{KpiQueryContext, KpiRegistry, KpiValue};
use crate::application::service::unsubscribe_service::UnsubscribeService;

/// The render's mail-facing strings for the three windows.
pub const WINDOW_LABELS: [&str; 3] = ["Last 24 hours", "Last 7 days", "Last 30 days"];

/// One recipient subscription row as the sweep reads it.
#[derive(Debug, Clone)]
pub struct RecipientRow {
    pub user_id: Uuid,
    pub metadata: Value,
}

impl RecipientRow {
    /// The growth loop's marker: this subscription was minted by the
    /// auto-subscribe handler (R-DG10's visible-unsubscribe-leg rule).
    pub fn auto_subscribed(&self) -> bool {
        self.metadata.get("auto_subscribed").and_then(Value::as_bool).unwrap_or(false)
    }

    /// Has this recipient received their FIRST digest mail yet?
    pub fn first_digest_sent_at(&self) -> Option<&str> {
        self.metadata.get("first_digest_sent_at").and_then(Value::as_str)
    }
}

/// The active recipients of one digest (state = subscribed), ordered.
pub async fn active_recipients(
    pool: &PgPool,
    digest_id: Uuid,
) -> Result<Vec<RecipientRow>, DigestError> {
    let rows = sqlx::query_as::<_, (Uuid, Value)>(
        r#"SELECT user_id, metadata
           FROM digest.digest_subscriptions
           WHERE digest_id = $1
             AND state = 'subscribed'
             AND (metadata->>'deleted_at') IS NULL
           ORDER BY user_id"#,
    )
    .bind(digest_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(user_id, metadata)| RecipientRow { user_id, metadata })
        .collect())
}

/// One KPI's rendered cells: the current value per window plus the
/// previous-period margin when it applies.
#[derive(Debug, Clone)]
pub struct RenderedKpi {
    pub key: String,
    pub label: String,
    /// Per window: the current value (always present — a genuine zero
    /// renders `0`) and the margin when shown.
    pub cells: [(KpiValue, Option<f64>); 3],
}

/// The carousel tip as picked and sanitized.
#[derive(Debug, Clone)]
pub struct TipPick {
    pub tip_id: Uuid,
    pub name: String,
    pub sanitized_html: String,
}

/// The full render plan for one recipient — everything the mail needs,
/// nothing sent yet. Probes assert the drop decision HERE (rendered vs
/// dropped keys) before any enqueue runs.
#[derive(Debug, Clone)]
pub struct RenderPlan {
    pub digest_id: Uuid,
    pub digest_name: String,
    pub user_id: Uuid,
    pub email_to: String,
    pub subject: String,
    pub body_html: String,
    pub headers: Value,
    /// Keys that rendered (in enablement order).
    pub rendered: Vec<String>,
    /// Keys dropped by the DECLARED fence (absent from the mail; listed
    /// here for tests/telemetry only — the mail never mentions them).
    pub dropped_out_of_fence: Vec<String>,
    /// Keys dropped because their source was unavailable (port unwired /
    /// source missing) — warned, never fatal.
    pub dropped_unavailable: Vec<String>,
    pub tip: Option<TipPick>,
    /// TRUE when this mail must carry the first-digest unsubscribe
    /// notice visibly (auto-subscribed, never mailed).
    pub is_first_digest_for_auto_subscriber: bool,
}

/// The render + delivery engine.
pub struct DigestRenderService {
    pool: PgPool,
    registry: Arc<KpiRegistry>,
    port: RecipientContextSlot,
    mail_seam: Arc<dyn DigestMailSeam>,
    /// None when the HMAC secret was never configured — renders then
    /// refuse LOUDLY per digest (a digest mail without the unsubscribe
    /// leg must not send), never silently omit the leg.
    unsub: Option<Arc<UnsubscribeService>>,
    public_base_url: String,
}

impl DigestRenderService {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        pool: PgPool,
        registry: Arc<KpiRegistry>,
        port: RecipientContextSlot,
        mail_seam: Arc<dyn DigestMailSeam>,
        unsub: Option<Arc<UnsubscribeService>>,
        public_base_url: impl Into<String>,
    ) -> Self {
        Self {
            pool,
            registry,
            port,
            mail_seam,
            unsub,
            public_base_url: public_base_url.into(),
        }
    }

    /// The unsubscribe URL for one (digest, user) — the CORRECT RFC 8058
    /// spelling (upstream's `unsubscribe_oneclik` typo does not port;
    /// recorded deviation). A fresh token per mail. Refuses LOUDLY when
    /// the public base URL is empty: a link minted on a guessed default
    /// host (a localhost fallback flowing into a production overlay)
    /// would strand every recipient's one-click exit — same posture as
    /// the missing-secret refusal.
    fn unsubscribe_url(&self, digest_id: Uuid, user_id: Uuid) -> Result<String, DigestError> {
        let Some(unsub) = self.unsub.as_ref() else {
            return Err(DigestError::SecretNotConfigured);
        };
        if self.public_base_url.trim().is_empty() {
            return Err(DigestError::PublicBaseUrlNotConfigured);
        }
        let token = unsub.mint_link_token(digest_id, user_id);
        Ok(format!("{}/digest/unsubscribe?t={token}", self.public_base_url.trim_end_matches('/')))
    }

    /// Render one recipient's mail (no enqueue). This is the
    /// condition-16 decision point.
    pub async fn render(
        &self,
        digest: &DigestRow,
        recipient: &RecipientRow,
        now: DateTime<Utc>,
    ) -> Result<RenderPlan, DigestError> {
        // The recipient's OWN fence — the port's active-company
        // resolution (the module's only sapiens window).
        let ctx = self
            .port
            .resolve(recipient.user_id)
            .await
            .map_err(|e| DigestError::SendFailed(format!(
                "recipient {} unresolved through the engagement port (fail-closed): {e}",
                recipient.user_id
            )))?;

        let url = self.unsubscribe_url(digest.id, recipient.user_id)?;

        // Enabled keys in stable order.
        let keys: Vec<String> = sqlx::query_scalar(
            r#"SELECT kpi_key FROM digest.digest_digest_kpis
               WHERE digest_id = $1 AND (metadata->>'deleted_at') IS NULL
               ORDER BY kpi_key"#,
        )
        .bind(digest.id)
        .fetch_all(&self.pool)
        .await?;

        let windows = windows_at(now);
        let mut rendered: Vec<RenderedKpi> = Vec::new();
        let mut dropped_out_of_fence: Vec<String> = Vec::new();
        let mut dropped_unavailable: Vec<String> = Vec::new();

        for key in keys {
            let Some(def) = self.registry.get(&key) else {
                // Config drift: the key was enabled against a registry
                // that has since changed. Loud warning, absent render —
                // never a mail failure.
                tracing::warn!(
                    digest_id = %digest.id,
                    kpi = %key,
                    "enabled KPI key absent from the composed registry (kpi_not_registered drift); dropping from this render"
                );
                dropped_unavailable.push(key);
                continue;
            };
            // ---- THE CONDITION-16 DECISION --------------------------------
            // From the entry's OWN fence declaration, BEFORE any compute
            // runs. Never from row count: under RLS an out-of-fence read
            // is zero rows — indistinguishable from a genuine zero. The
            // recipient's resolved company is the only fence leg — the
            // module ships no tenancy axis (ADR-0029), so which digests a
            // recipient reaches at all is the composer's org fence.
            if !def.fence.renders_for(ctx.company_id) {
                dropped_out_of_fence.push(key);
                continue;
            }
            // In fence: compute all three windows; a genuine zero is a
            // VALUE here, not a drop.
            let mut cells: Vec<(KpiValue, Option<f64>)> = Vec::with_capacity(3);
            let mut unavailable = false;
            for (cur_w, prev_w) in windows.iter() {
                let cur = self
                    .compute_one(&def.computer, recipient.user_id, ctx.company_id, *cur_w)
                    .await;
                let prev = self
                    .compute_one(&def.computer, recipient.user_id, ctx.company_id, *prev_w)
                    .await;
                match (cur, prev) {
                    (Ok(cur), Ok(prev)) => {
                        let m = margin(cur.display(), prev.display());
                        cells.push((cur, m))
                    }
                    (Err(Ok(why)), _) | (_, Err(Ok(why))) => {
                        // Benign: source not present in this composition.
                        // Warn and drop the KPI for THIS render — never
                        // fail the recipient's mail over one metric.
                        tracing::warn!(kpi = %key, kpi_source_unavailable = %why, "KPI source unavailable; dropping from this render");
                        unavailable = true;
                        break;
                    }
                    (Err(Err(e)), _) | (_, Err(Err(e))) => return Err(e),
                }
            }
            if unavailable {
                dropped_unavailable.push(key);
                continue;
            }
            let [c0, c1, c2] = cells.try_into().expect("exactly three windows");
            rendered.push(RenderedKpi {
                key: key.clone(),
                label: def.label.clone(),
                cells: [c0, c1, c2],
            });
        }

        let tip = self.pick_tip(recipient.user_id).await?;
        let is_first_digest_for_auto_subscriber =
            recipient.auto_subscribed() && recipient.first_digest_sent_at().is_none();

        let subject = format!("{} — {}", digest.name, now.date_naive());
        let body_html = build_body(
            digest,
            &rendered,
            &tip,
            &url,
            is_first_digest_for_auto_subscriber,
        );

        let headers = serde_json::json!({
            // RFC 8058's one-click pair (the correct spelling; CRLF-free
            // values — mail's enqueue refuses them, verified upstream).
            "List-Unsubscribe": format!("<{url}>"),
            "List-Unsubscribe-Post": "List-Unsubscribe=One-Click",
            "X-Auto-Response-Suppress": "OOF",
        });

        Ok(RenderPlan {
            digest_id: digest.id,
            digest_name: digest.name.clone(),
            user_id: recipient.user_id,
            email_to: ctx.email,
            subject,
            body_html,
            headers,
            rendered: rendered.iter().map(|k| k.key.clone()).collect(),
            dropped_out_of_fence,
            dropped_unavailable,
            tip,
            is_first_digest_for_auto_subscriber,
        })
    }

    /// One compute over one window. `Err(String)` = the source is
    /// unavailable (benign — the KPI drops from this render); a
    /// `DigestError` = infrastructure trouble (isolated per digest).
    async fn compute_one(
        &self,
        computer: &Arc<dyn crate::application::service::kpi_registry::KpiComputer>,
        recipient_user_id: Uuid,
        recipient_company: Option<Uuid>,
        window: TimeWindow,
    ) -> Result<KpiValue, Result<String, DigestError>> {
        let qctx = KpiQueryContext {
            recipient_user_id,
            // The recipient's OWN fence — `None` when they carry no active
            // company (company-scoped computes were already dropped at the
            // fence; shared-data computes ignore the value).
            company_id: recipient_company,
            window_start: window.start,
            window_end: window.end,
        };
        match computer.compute(&self.pool, &qctx).await {
            Ok(v) => Ok(v),
            Err(crate::application::service::kpi_registry::KpiError::Unavailable(why)) => {
                Err(Ok(why))
            }
            Err(e) => Err(Err(DigestError::SendFailed(e.to_string()))),
        }
    }

    /// The carousel pick: lowest-sequence unconsumed tip the recipient's
    /// group keys pass. The group gate resolves through the port and
    /// FAILS CLOSED (a gated tip with an unwired port does not render;
    /// NULL-gate tips always can).
    async fn pick_tip(&self, user_id: Uuid) -> Result<Option<TipPick>, DigestError> {
        let rows = sqlx::query_as::<_, (Uuid, String, Option<String>, Option<String>)>(
            r#"SELECT id, name, tip_description, group_key
               FROM digest.digest_tips
               WHERE (metadata->>'deleted_at') IS NULL
                 AND NOT EXISTS (
                     SELECT 1 FROM digest.digest_tip_users tu
                     WHERE tu.tip_id = digest.digest_tips.id
                       AND tu.user_id = $1
                       AND (tu.metadata->>'deleted_at') IS NULL
                 )
               ORDER BY sequence, id"#,
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;

        let keys = match self.port.group_keys(user_id).await {
            Ok(k) => Some(k),
            Err(e) => {
                tracing::warn!(
                    group_key_resolution = %e,
                    "tip group-key resolution failed; failing closed to ungated tips only"
                );
                None
            }
        };

        for (tip_id, name, description, group_key) in rows {
            let gated = group_key.unwrap_or_default();
            if !gated.is_empty() {
                match &keys {
                    Some(set) if set.contains(&gated) => {}
                    // Gate not passed (or unwired): skip this tip, keep
                    // walking the carousel.
                    _ => continue,
                }
            }
            // Stored AS AUTHORED; sanitized HERE (R-DGT1) — ammonia, the
            // recorded sanitizer choice (none exists in the mail stack).
            let sanitized_html = ammonia::clean(&description.unwrap_or_default());
            return Ok(Some(TipPick { tip_id, name, sanitized_html }));
        }
        Ok(None)
    }

    /// Deliver a render plan: enqueue through the mail seam, THEN write
    /// the tip consumption marker, THEN stamp the first-digest marker —
    /// in that order, on a successful enqueue only (R-DGT1: Odoo burns
    /// the tip before the enqueue; that weaker order does not port).
    /// Returns the enqueued mail id.
    pub async fn deliver(&self, plan: &RenderPlan) -> Result<Uuid, DigestError> {
        let mail = OutgoingDigestMail {
            digest_id: plan.digest_id,
            recipient_user_id: plan.user_id,
            email_to: plan.email_to.clone(),
            subject: plan.subject.clone(),
            body_html: plan.body_html.clone(),
            headers: plan.headers.clone(),
            model: "digest".into(),
            queued_at: Utc::now(),
        };
        let mail_id = self.mail_seam.send_digest_mail(&mail).await?;

        if let Some(tip) = &plan.tip {
            sqlx::query(
                r#"INSERT INTO digest.digest_tip_users (tip_id, user_id)
                   VALUES ($1, $2)
                   ON CONFLICT (tip_id, user_id)
                   DO NOTHING"#,
            )
            .bind(tip.tip_id)
            .bind(plan.user_id)
            .execute(&self.pool)
            .await?;
        }

        if plan.is_first_digest_for_auto_subscriber {
            // First mail to a growth-loop subscriber: stamp the marker so
            // later mails drop the extra notice (the unsubscribe leg
            // stays in every footer regardless).
            sqlx::query(
                r#"UPDATE digest.digest_subscriptions
                   SET metadata = metadata || $3::jsonb
                   WHERE digest_id = $1 AND user_id = $2"#,
            )
            .bind(plan.digest_id)
            .bind(plan.user_id)
            .bind(serde_json::json!({ "first_digest_sent_at": Utc::now().to_rfc3339() }))
            .execute(&self.pool)
            .await?;
        }

        tracing::info!(
            target: "digest::audit",
            event = "digest_email_sent",
            digest_id = %plan.digest_id,
            user_id = %plan.user_id,
            mail_message_id = %mail_id,
            email_to = %plan.email_to
        );
        Ok(mail_id)
    }

    /// Render + deliver one recipient (the sweep's step 3 body).
    pub async fn send_to_recipient(
        &self,
        digest: &DigestRow,
        recipient: &RecipientRow,
        now: DateTime<Utc>,
    ) -> Result<(RenderPlan, Uuid), DigestError> {
        let plan = self.render(digest, recipient, now).await?;
        let mail_id = self.deliver(&plan).await?;
        Ok((plan, mail_id))
    }
}

/// The plain-text-ish HTML body. Deliberately small: the mail renders a
/// title, the 3-window KPI table, the tip, and the visible unsubscribe
/// footer (R-DG7/R-DG10's visible-leg rule). Every authored string
/// (digest name, metric label, tip) crosses `ammonia::clean` — the
/// HTML sanitizer (NOT `clean_text`, which entity-escapes plain text
/// for text/plain contexts).
fn build_body(
    digest: &DigestRow,
    rendered: &[RenderedKpi],
    tip: &Option<TipPick>,
    unsubscribe_url: &str,
    first_digest_notice: bool,
) -> String {
    use std::fmt::Write as _;
    let mut b = String::new();
    let _ = writeln!(
        b,
        "<html><body>\
         <h2>{}</h2>\
         <p>Periodic KPI digest for {}.</p>",
        ammonia::clean(&digest.name),
        Utc::now().date_naive()
    );

    if first_digest_notice {
        let _ = writeln!(
            b,
            "<p>You are receiving this digest because an account was created for you. \
             You can unsubscribe at any time using the link at the bottom of this email.</p>"
        );
    }

    if rendered.is_empty() {
        let _ = writeln!(b, "<p>No metrics to show this period.</p>");
    } else {
        let _ = writeln!(
            b,
            "<table border=\"0\" cellpadding=\"4\" cellspacing=\"0\">\
             <tr><th align=\"left\">Metric</th>"
        );
        for label in WINDOW_LABELS {
            let _ = write!(b, "<th align=\"right\">{label}</th>");
        }
        let _ = writeln!(b, "</tr>");
        for kpi in rendered {
            let _ = write!(b, "<tr><td>{}</td>", ammonia::clean(&kpi.label));
            for (value, margin_pct) in &kpi.cells {
                let mut cell = match value.amount {
                    Some(a) => format!("{a:.2}"),
                    None => format!("{}", value.count),
                };
                if let Some(pct) = margin_pct {
                    cell.push_str(&format!(" ({pct:+}%)"));
                }
                let _ = write!(b, "<td align=\"right\">{cell}</td>");
            }
            let _ = writeln!(b, "</tr>");
        }
        let _ = writeln!(b, "</table>");
    }

    if let Some(t) = tip {
        let _ = writeln!(
            b,
            "<div class=\"digest-tip\"><strong>Tip: {}</strong><p>{}</p></div>",
            ammonia::clean(&t.name),
            t.sanitized_html
        );
    }

    let _ = writeln!(
        b,
        "<hr><p><a href=\"{unsubscribe_url}\">Unsubscribe from this digest in one click</a></p>\
         </body></html>"
    );
    b
}

/// Assert (compile-time visibility) that the base KPI names carry the
/// registry prefix — the namespace the enablement rows reference.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_labels_match_spans() {
        assert_eq!(WINDOW_LABELS.len(), crate::application::service::base_kpis::WINDOW_SPANS_DAYS.len());
        assert!(crate::application::service::kpi_registry::KPI_CONNECTED_USERS.starts_with("kpi_"));
        assert!(crate::application::service::kpi_registry::KPI_MESSAGES_SENT.starts_with("kpi_"));
    }

    #[test]
    fn body_carries_the_unsubscribe_leg_visibly() {
        let digest = DigestRow {
            id: Uuid::new_v4(),
            name: "Ops Digest".into(),
            periodicity: crate::application::service::digest_write_service::DigestPeriodicity::Daily,
            next_run_date: None,
            state: "activated".into(),
        };
        let body = build_body(&digest, &[], &None, "https://mail.example.test/digest/unsubscribe?t=abc", false);
        assert!(body.contains("Unsubscribe from this digest in one click"));
        assert!(!body.contains("because an account was created"));
        let first = build_body(&digest, &[], &None, "https://x/digest/unsubscribe?t=abc", true);
        assert!(first.contains("because an account was created"));
    }
}
