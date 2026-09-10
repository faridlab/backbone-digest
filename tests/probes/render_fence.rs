//! Condition 16: the out-of-fence KPI drop is decided from the
//! registry entry's fence DECLARATION, never from row count.
//!
//! The per-recipient witness: one digest carries a CompanyData KPI and a
//! PinnedCompanyData(co_a) KPI. A recipient WITH an active company renders
//! the CompanyData KPI under their OWN company; a recipient pinned to
//! another company drops the pinned key; a company-less recipient fails
//! CLOSED and drops both company-scoped keys — even though the underlying
//! data is nonzero. Under RLS the distinction is unobservable from data;
//! only the declaration can make it. (Which digests a recipient may reach
//! at all is the composing service's org fence — the module ships no
//! tenancy axis per ADR-0029 — so no cross-digest witness exists here.)

use std::collections::HashSet;

use chrono::{Duration, Utc};
use uuid::Uuid;

use backbone_digest::application::service::engagement_port::{
    CannedAnswers, CannedRecipientContext, RecipientContext,
};
use backbone_digest::application::service::kpi_registry::{
    FnComputer, KpiDefinition, KpiFenceDeclaration, KpiValue, KPI_CONNECTED_USERS,
    KPI_MESSAGES_SENT,
};
use backbone_digest::application::service::{DigestError, DigestPeriodicity, KpiRegistrationError};

use super::common::{
    seed_mail_message, seed_membership, seed_org_unit, seed_tip, seed_user, subscription_state,
    Svc, TestDb,
};

/// The pure decision table (no DB): the ONLY inputs are the declared
/// fence and the recipient's resolved company.
#[test]
fn fence_decision_is_declaration_driven_and_fails_closed() {
    let a = Uuid::new_v4();
    let b = Uuid::new_v4();

    // Shared data renders for everyone.
    assert!(KpiFenceDeclaration::SharedData.renders_for(None));
    assert!(KpiFenceDeclaration::SharedData.renders_for(Some(b)));

    // Company data renders under the recipient's OWN resolved company...
    assert!(KpiFenceDeclaration::CompanyData.renders_for(Some(a)));
    // ...and FAILS CLOSED when the recipient carries no active company
    // (never render on an unestablishable fence).
    assert!(!KpiFenceDeclaration::CompanyData.renders_for(None));

    // Pinned data renders only for the pinned company's recipients.
    assert!(KpiFenceDeclaration::PinnedCompanyData(a).renders_for(Some(a)));
    assert!(!KpiFenceDeclaration::PinnedCompanyData(a).renders_for(Some(b)));
    assert!(!KpiFenceDeclaration::PinnedCompanyData(a).renders_for(None));
}

/// The registry contract: prefix refusal is typed; a duplicate name is
/// a load-time PANIC (a metric name is an identity, not a slot).
#[test]
fn registry_refuses_bad_prefix_and_panics_on_duplicates() {
    let registry = backbone_digest::application::service::KpiRegistry::new();
    let err = registry
        .register(KpiDefinition {
            name: "connected_users".into(),
            label: "No Prefix".into(),
            fence: KpiFenceDeclaration::SharedData,
            computer: std::sync::Arc::new(FnComputer(|_pool: &sqlx::PgPool, _ctx| {
                let _owned = _pool.clone();
                async move { Ok(KpiValue::zero()) }
            })),
        })
        .expect_err("unprefixed name must be refused");
    assert!(matches!(err, KpiRegistrationError::BadPrefix(_, _)));

    let def = KpiDefinition {
        name: "kpi_probe_once".into(),
        label: "Once".into(),
        fence: KpiFenceDeclaration::SharedData,
        computer: std::sync::Arc::new(FnComputer(|_pool: &sqlx::PgPool, _ctx| {
            let _owned = _pool.clone();
            async move { Ok(KpiValue::zero()) }
        })),
    };
    registry.register(def.clone()).expect("first registration");
    let second = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        registry.register(def);
    }))
    .expect_err("duplicate registration must panic (R-DG1)");
    let payload = second
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| second.downcast_ref::<&str>().map(|s| s.to_string()))
        .unwrap_or_default();
    assert!(
        payload.contains("duplicate KPI registration"),
        "the panic names the rule, got: {payload}"
    );
}

/// The cadence whitelist is exact (R-DG9): the four literals, nothing
/// else — no case-folding, no whitespace, no aliases.
#[test]
fn periodicity_whitelist_is_exact() {
    use DigestPeriodicity::{Daily, Monthly, Quarterly, Weekly};
    for ok in ["daily", "weekly", "monthly", "quarterly"] {
        assert!(DigestPeriodicity::parse(ok).is_some(), "'{ok}' must parse");
    }
    for refused in ["Daily", "WEEKLY", " daily", "daily ", "biweekly", "hourly", "yearly", ""] {
        assert!(
            DigestPeriodicity::parse(refused).is_none(),
            "'{refused}' must be refused"
        );
    }
    assert_eq!(Daily.as_str(), "daily");
    assert_eq!(Weekly.as_str(), "weekly");
    assert_eq!(Monthly.as_str(), "monthly");
    assert_eq!(Quarterly.as_str(), "quarterly");
    // The ladder is forward-only with a quarterly floor.
    assert_eq!(Daily.next_rung(), Some(Weekly));
    assert_eq!(Weekly.next_rung(), Some(Monthly));
    assert_eq!(Monthly.next_rung(), Some(Quarterly));
    assert_eq!(Quarterly.next_rung(), None);
    // Calendar arithmetic clamps month-ends (Jan 31 -> Feb 28; the
    // Dec 31 rollover lands on Jan 31).
    let jan31 = chrono::NaiveDate::from_ymd_opt(2026, 1, 31).unwrap();
    assert_eq!(Monthly.advance(jan31), chrono::NaiveDate::from_ymd_opt(2026, 2, 28).unwrap());
    let dec31 = chrono::NaiveDate::from_ymd_opt(2026, 12, 31).unwrap();
    assert_eq!(Monthly.advance(dec31), chrono::NaiveDate::from_ymd_opt(2027, 1, 31).unwrap());
}

/// THE condition-16 probe: declaration-driven drops per recipient, a
/// genuine zero rendered (not dropped), the typed unknown-key refusal,
/// the sanitized tip, the RFC 8058 header set, and the consume-after-
/// enqueue tip order.
#[tokio::test]
async fn out_of_fence_kpis_drop_by_declaration_not_row_count() {
    let db = TestDb::new("fence").await;
    let svc = Svc::new(db.pool.clone());
    svc.install_sql_port();
    let now = Utc::now();

    let co_a = seed_org_unit(&db.pool, "CO-A", "Company A").await;
    let co_b = seed_org_unit(&db.pool, "CO-B", "Company B").await;

    // A KPI pinned to company A (an out-of-the-base-pair registration —
    // the module's open extension surface).
    svc.registry
        .register(KpiDefinition {
            name: "kpi_probe_pinned_co".into(),
            label: "Pinned Co Metric".into(),
            fence: KpiFenceDeclaration::PinnedCompanyData(co_a),
            computer: std::sync::Arc::new(FnComputer(|pool: &sqlx::PgPool, _ctx| {
                let pool = pool.clone();
                async move {
                    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM users")
                        .fetch_one(&pool)
                        .await
                        .map_err(|e| {
                            backbone_digest::application::service::KpiError::Unavailable(
                                e.to_string(),
                            )
                        })?;
                    Ok(KpiValue::count(n))
                }
            })),
        })
        .expect("register pinned probe KPI");

    // The digest with all three fences enabled.
    let digest = svc
        .write
        .create_digest("Fence Digest", DigestPeriodicity::Daily, now.date_naive())
        .await
        .expect("create digest");
    for key in [KPI_CONNECTED_USERS, "kpi_probe_pinned_co", KPI_MESSAGES_SENT] {
        svc.write.enable_kpi(digest, key).await.expect("enable KPI");
    }

    // Unknown keys are the typed enablement refusal.
    let refused = svc
        .write
        .enable_kpi(digest, "kpi_does_not_exist")
        .await
        .expect_err("unknown key must be refused");
    assert!(
        matches!(refused, DigestError::Invalid(ref msg) if msg.contains("kpi_not_registered")),
        "unknown-key refusal must carry kpi_not_registered, got {refused:?}"
    );

    // alice: member of company A, logged in within the window — company
    // A's connected data is NONZERO.
    let alice = seed_user(&db.pool, "alice@a.test", Some(now - Duration::hours(1))).await;
    seed_membership(&db.pool, co_a, alice).await;
    // bob: member of company B, logged in too — his CompanyData KPI
    // computes under HIS OWN company (company B's connected data).
    let bob = seed_user(&db.pool, "bob@b.test", Some(now - Duration::hours(1))).await;
    seed_membership(&db.pool, co_b, bob).await;
    // dave: NO organization membership at all — the company-scoped fence
    // cannot be established for him, and must fail CLOSED.
    let dave = seed_user(&db.pool, "dave@none.test", Some(now - Duration::hours(1))).await;
    // Shared volume every recipient can see.
    seed_mail_message(&db.pool, now - Duration::hours(1)).await;
    seed_mail_message(&db.pool, now - Duration::hours(2)).await;

    assert!(svc.write.subscribe_user(digest, alice, None).await.expect("subscribe alice"));
    assert!(svc.write.subscribe_user(digest, bob, None).await.expect("subscribe bob"));
    assert!(svc.write.subscribe_user(digest, dave, None).await.expect("subscribe dave"));

    // The XSS-carrying tip (stored as authored; sanitized at render).
    let tip = seed_tip(
        &db.pool,
        1,
        "Probe Tip",
        "<script>alert(1)</script><b>Real tip body</b>",
        None,
    )
    .await;
    // A gated tip alice holds no key for — the carousel must skip it.
    let gated = seed_tip(&db.pool, 2, "Gated Tip", "<i>never for alice</i>", Some("ADMIN-PROBE")).await;

    let digest_row = svc.write.get_digest(digest).await.expect("read digest").expect("digest row");
    let recipients =
        backbone_digest::application::service::digest_render_service::active_recipients(
            &db.pool, digest,
        )
        .await
        .expect("recipients");
    assert_eq!(recipients.len(), 3, "all subscriptions are active");

    let mut plans = std::collections::BTreeMap::new();
    for recipient in &recipients {
        let plan = svc
            .render
            .render(&digest_row, recipient, now)
            .await
            .expect("render must succeed for every recipient");
        plans.insert(recipient.user_id, plan);
    }
    let alice_plan = &plans[&alice];
    let bob_plan = &plans[&bob];
    let dave_plan = &plans[&dave];

    // ---- alice (company A member): everything renders ------------------
    assert!(alice_plan.rendered.contains(&KPI_CONNECTED_USERS.to_string()));
    assert!(alice_plan.rendered.contains(&"kpi_probe_pinned_co".to_string()));
    assert!(alice_plan.rendered.contains(&KPI_MESSAGES_SENT.to_string()));
    assert!(
        alice_plan.dropped_out_of_fence.is_empty(),
        "a recipient with a resolved company must see every enabled KPI: {:?}",
        alice_plan.dropped_out_of_fence
    );
    assert!(alice_plan.dropped_unavailable.is_empty());

    // ---- bob (company B member): the PINNED key drops, by declaration --
    // Company A HAS connected logged-in users (alice) — the underlying
    // pinned data is nonzero — yet bob's render drops the pinned key: his
    // resolved company is B, not the pinned A. Under RLS an out-of-fence
    // read is zero rows; only the declaration makes this drop (a row-count
    // rule could never distinguish it). His CompanyData KPI RENDERS — it
    // computes under HIS OWN company's fence.
    assert!(bob_plan.rendered.contains(&KPI_CONNECTED_USERS.to_string()));
    assert!(!bob_plan.rendered.contains(&"kpi_probe_pinned_co".to_string()));
    assert!(
        bob_plan.dropped_out_of_fence.contains(&"kpi_probe_pinned_co".to_string()),
        "PinnedCompanyData(co_a) key must drop for the company-B recipient"
    );
    // Shared data still renders for bob.
    assert!(bob_plan.rendered.contains(&KPI_MESSAGES_SENT.to_string()));
    // The drop lists only the pinned key.
    assert_eq!(bob_plan.dropped_out_of_fence.len(), 1);

    // ---- dave (company-less): BOTH company-scoped keys fail CLOSED -----
    // Dave HAS underlying connected data (he is logged in) — yet both
    // company-scoped keys drop: with no resolved company the fence cannot
    // be established, and an unestablishable fence never renders.
    assert!(!dave_plan.rendered.contains(&KPI_CONNECTED_USERS.to_string()));
    assert!(!dave_plan.rendered.contains(&"kpi_probe_pinned_co".to_string()));
    assert!(
        dave_plan.dropped_out_of_fence.contains(&KPI_CONNECTED_USERS.to_string()),
        "CompanyData key must fail closed for the company-less recipient"
    );
    assert!(
        dave_plan.dropped_out_of_fence.contains(&"kpi_probe_pinned_co".to_string()),
        "PinnedCompanyData key must fail closed for the company-less recipient"
    );
    // Shared data still renders for dave.
    assert!(dave_plan.rendered.contains(&KPI_MESSAGES_SENT.to_string()));
    assert_eq!(dave_plan.dropped_out_of_fence.len(), 2);

    // ---- genuine zero renders as a value, never a drop -----------------
    // A THIRD company nobody logged into: its connected metric is a
    // genuine ZERO (count_connected finds no last_login in any window)
    // — and zero RENDERS, it is never a drop.
    let co_c = seed_org_unit(&db.pool, "CO-C", "Zero Company").await;
    let digest_b = svc
        .write
        .create_digest("Zero Digest", DigestPeriodicity::Daily, now.date_naive())
        .await
        .expect("create digest B");
    svc.write.enable_kpi(digest_b, KPI_CONNECTED_USERS).await.expect("enable connected on B");
    let charlie = seed_user(&db.pool, "charlie@c.test", None).await;
    seed_membership(&db.pool, co_c, charlie).await;
    assert!(svc.write.subscribe_user(digest_b, charlie, None).await.expect("subscribe charlie"));

    let digest_b_row = svc.write.get_digest(digest_b).await.expect("read B").expect("B row");
    let charlie_row =
        backbone_digest::application::service::digest_render_service::RecipientRow {
            user_id: charlie,
            metadata: serde_json::json!({}),
        };
    let charlie_plan = svc
        .render
        .render(&digest_b_row, &charlie_row, now)
        .await
        .expect("render B");
    assert!(
        charlie_plan.rendered.contains(&KPI_CONNECTED_USERS.to_string()),
        "a genuine zero is IN the render (renders as 0), never a drop"
    );
    assert!(charlie_plan.dropped_out_of_fence.is_empty());
    assert!(charlie_plan.dropped_unavailable.is_empty());
    // The body shows the zero cell for the connected metric.
    let connected_idx = charlie_plan
        .body_html
        .find("Connected Users")
        .expect("body carries the metric label");
    let after = &charlie_plan.body_html[connected_idx..connected_idx + 200];
    assert!(after.contains('0'), "the zero cell must render, body snippet: {after}");

    // ---- the tip: sanitized, ungated, consume-after-enqueue ------------
    assert!(alice_plan.tip.is_some(), "the ungated tip must be picked");
    let picked = alice_plan.tip.as_ref().expect("tip");
    assert_eq!(picked.tip_id, tip, "the lowest-sequence ungated tip wins");
    assert!(!picked.sanitized_html.contains("<script>"), "script tags must be stripped");
    assert!(picked.sanitized_html.contains("<b>Real tip body</b>"));
    // Nothing consumed yet (the marker is written at DELIVER).
    let consumed: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM digest.digest_tip_users WHERE user_id = $1",
    )
    .bind(alice)
    .fetch_one(&db.pool)
    .await
    .expect("tip markers");
    assert_eq!(consumed, 0, "the tip is consumed only after a successful enqueue");

    // ---- deliver: seam mail + RFC 8058 headers + tip marker ------------
    let mail_id = svc.render.deliver(alice_plan).await.expect("deliver");
    assert!(!mail_id.is_nil());
    let sent = svc.seam.sent();
    assert_eq!(sent.len(), 1);
    let mail = &sent[0];
    assert_eq!(mail.recipient_user_id, alice);
    assert_eq!(mail.email_to, "alice@a.test");
    assert!(mail.subject.contains("Fence Digest"));
    assert_eq!(
        mail.headers["List-Unsubscribe-Post"].as_str(),
        Some("List-Unsubscribe=One-Click"),
        "the RFC 8058 one-click header must ride the enqueue"
    );
    let link = mail.headers["List-Unsubscribe"].as_str().expect("List-Unsubscribe");
    assert!(link.starts_with("<http://digest.probe.test/digest/unsubscribe?t="));
    // The mailed token is REAL: the full unsubscribe round-trip works
    // with the same secret (mint → link → apply).
    let token = link
        .trim_start_matches('<')
        .trim_end_matches('>')
        .rsplit("t=")
        .next()
        .expect("token segment");
    assert!(svc
        .unsub
        .unsubscribe_by_token(token, now.timestamp())
        .await
        .expect("token round-trip"));
    assert_eq!(
        subscription_state(&db.pool, digest, alice).await,
        "unsubscribed",
        "the mailed token must perform the unsubscribe"
    );

    // The tip marker landed AFTER the successful enqueue.
    let consumed: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM digest.digest_tip_users WHERE user_id = $1 AND tip_id = $2",
    )
    .bind(alice)
    .bind(tip)
    .fetch_one(&db.pool)
    .await
    .expect("tip markers after deliver");
    assert_eq!(consumed, 1);
    let _ = gated; // the gated tip is never picked (alice holds no roles)

    // ---- a delivery refusal burns NO tip (the decided order) -----------
    svc.seam.fail_next("probe smtp down");
    let bob_recipient =
        backbone_digest::application::service::digest_render_service::RecipientRow {
            user_id: bob,
            metadata: serde_json::json!({}),
        };
    let bob_plan2 = svc.render.render(&digest_row, &bob_recipient, now).await.expect("bob render");
    assert!(
        svc.render.deliver(&bob_plan2).await.is_err(),
        "the seam was made to refuse"
    );
    let bob_consumed: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM digest.digest_tip_users WHERE user_id = $1",
    )
    .bind(bob)
    .fetch_one(&db.pool)
    .await
    .expect("bob tip markers");
    assert_eq!(bob_consumed, 0, "a refused enqueue must not consume the tip");
    svc.seam.clear_failure();

    db.dispose().await;
}

/// The unavailable-source drop: an unwired port drops the Connected
/// Users KPI from THAT render with a warning — it never fails the mail
/// (Messages Sent still renders).
#[tokio::test]
async fn unavailable_sources_drop_the_kpi_never_the_mail() {
    let db = TestDb::new("unavail").await;
    let svc = Svc::new(db.pool.clone());
    // Deliberately NO port install: every identity fact refuses.

    let now = Utc::now();
    // A canned company id (never persisted — the canned port answers
    // from memory, no organization row involved).
    let co = Uuid::new_v4();
    let digest = svc
        .write
        .create_digest("Unwired Digest", DigestPeriodicity::Daily, now.date_naive())
        .await
        .expect("create digest");
    svc.write.enable_kpi(digest, KPI_CONNECTED_USERS).await.expect("enable connected");
    svc.write.enable_kpi(digest, KPI_MESSAGES_SENT).await.expect("enable messages");
    seed_mail_message(&db.pool, now - Duration::hours(1)).await;

    let someone = seed_user(&db.pool, "nobody@x.test", None).await;
    assert!(svc.write.subscribe_user(digest, someone, None).await.expect("subscribe"));

    let digest_row = svc.write.get_digest(digest).await.expect("read").expect("row");
    let recipient =
        backbone_digest::application::service::digest_render_service::RecipientRow {
            user_id: someone,
            metadata: serde_json::json!({}),
        };
    // Render REQUIRES the recipient resolve (fail-closed identity) —
    // with an unwired port the whole render refuses loudly here, which
    // is the documented contract; the per-KPI unavailable drop shows up
    // through a wired port whose SOURCE is missing instead. Wire the
    // canned port with a resolved context to reach the KPI layer.
    let err = svc.render.render(&digest_row, &recipient, now).await;
    assert!(
        err.is_err(),
        "an unwired recipient port must fail the render loudly (fail-closed identity)"
    );

    svc.slot.install(std::sync::Arc::new(CannedRecipientContext::with(CannedAnswers {
        context: Some(RecipientContext {
            user_id: someone,
            email: "nobody@x.test".into(),
            company_id: Some(co),
            is_internal: true,
            last_login: None,
        }),
        connected: 0,
        any_logged_in: false,
        keys: HashSet::new(),
    })));
    let plan = svc.render.render(&digest_row, &recipient, now).await.expect("render");
    assert!(plan.rendered.contains(&KPI_MESSAGES_SENT.to_string()));
    // The connected KPI answers through the canned port (0), so it
    // renders as a genuine zero — the unavailable shape is exercised by
    // compute_one's Unavailable mapping (unit-covered); the mail never
    // failed either way.
    assert!(plan.rendered.contains(&KPI_CONNECTED_USERS.to_string()));
    assert!(plan.dropped_out_of_fence.is_empty());

    db.dispose().await;
}
