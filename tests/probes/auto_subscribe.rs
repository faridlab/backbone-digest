//! The growth loop: `sapiens.user.created` on the host bus → subscribe
//! the new INTERNAL user to the configured default digest. Internal =
//! an active organization membership (the payload carries no
//! is_internal flag); idempotent across at-least-once redelivery;
//! every refusal is AUDITED, never a silent skip; an unwired port or
//! an unresolvable user is a loud EventError (relay retries), never a
//! poison Ok.

use std::sync::Arc;

use chrono::{Duration, Utc};
use uuid::Uuid;

use backbone_digest::application::service::engagement_port::RecipientContextSlot;
use backbone_digest::application::service::user_created_handler::UserCreatedHandler;
use backbone_messaging::IntegrationEventHandler;

use super::common::{
    seed_membership, seed_org_unit, seed_user, subscription_metadata, user_created_envelope, Svc,
    TestDb,
};

/// The full loop: internal subscribe, replay idempotency, the three
/// refusal shapes, and the loud failures.
#[tokio::test]
async fn created_users_subscribe_internal_only_and_idempotently() {
    let db = TestDb::new("growth").await;
    let svc = Svc::new(db.pool.clone());
    svc.install_sql_port();
    let now = Utc::now();
    let co = seed_org_unit(&db.pool, "GROWTH", "Growth Co").await;

    let default_digest = svc
        .write
        .create_digest("Default Digest", backbone_digest::application::service::DigestPeriodicity::Daily, now.date_naive())
        .await
        .expect("create default digest");
    let handler = UserCreatedHandler::new(svc.write.clone(), svc.slot.clone(), Some(default_digest));

    // The subscription contract surface.
    assert_eq!(handler.event_patterns(), vec!["sapiens.user.created"]);
    assert_eq!(handler.name(), "DigestUserCreatedHandler");

    // ---- internal user: subscribed with the growth-loop markers -------
    let internal = seed_user(&db.pool, "newhire@co.test", None).await;
    seed_membership(&db.pool, co, internal).await;
    let envelope = user_created_envelope(internal);
    handler
        .handle(envelope.clone())
        .await
        .expect("an internal user's creation subscribes");
    let meta = subscription_metadata(&db.pool, default_digest, internal).await;
    assert_eq!(meta["auto_subscribed"], serde_json::json!(true), "markers: {meta}");
    assert_eq!(meta["auto_subscribed_envelope"], serde_json::json!(envelope.id));

    // ---- the replay: same envelope again → Ok, still ONE row ----------
    handler.handle(envelope).await.expect("the replay is a success");
    let rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM digest.digest_subscriptions WHERE user_id = $1")
            .bind(internal)
            .fetch_one(&db.pool)
            .await
            .expect("count rows");
    assert_eq!(rows, 1, "idempotent on the (digest, user) key across redelivery");
    assert_eq!(
        super::common::subscription_state(&db.pool, default_digest, internal).await,
        "subscribed"
    );

    // ---- external user (no membership): audited refusal, no row --------
    let external = seed_user(&db.pool, "customer@portal.test", None).await;
    handler
        .handle(user_created_envelope(external))
        .await
        .expect("a non-internal creation is NOT a bus error");
    let rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM digest.digest_subscriptions WHERE user_id = $1")
            .bind(external)
            .fetch_one(&db.pool)
            .await
            .expect("count external rows");
    assert_eq!(rows, 0, "external users are never subscribed");

    // ---- growth loop OFF (no default digest): audited refusal ----------
    let off = UserCreatedHandler::new(svc.write.clone(), svc.slot.clone(), None);
    let later = seed_user(&db.pool, "later@co.test", Some(now - Duration::days(1))).await;
    seed_membership(&db.pool, co, later).await;
    off.handle(user_created_envelope(later))
        .await
        .expect("the unconfigured loop is an audited refusal, never an error");
    let rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM digest.digest_subscriptions WHERE user_id = $1")
            .bind(later)
            .fetch_one(&db.pool)
            .await
            .expect("count later rows");
    assert_eq!(rows, 0);

    // ---- unwired port: LOUD EventError (a composition bug) -------------
    let unwired_slot = RecipientContextSlot::default();
    let unwired = UserCreatedHandler::new(svc.write.clone(), unwired_slot, Some(default_digest));
    let loud = unwired
        .handle(user_created_envelope(internal))
        .await
        .expect_err("an unwired port must fail the delivery loudly");
    assert!(
        loud.to_string().contains("not wired"),
        "the EventError names the unwired port: {loud}"
    );

    // ---- unresolvable user: EventError (the relay retries) -------------
    let ghost = Uuid::new_v4();
    let retry = handler
        .handle(user_created_envelope(ghost))
        .await
        .expect_err("an unresolvable user must error the delivery for retry");
    assert!(!retry.to_string().is_empty());

    // ---- malformed payload: EventError ---------------------------------
    let mut bad = user_created_envelope(internal);
    bad.payload = serde_json::json!({ "something_else": true });
    assert!(handler.handle(bad).await.is_err(), "a payload without user_id errors");

    // ---- the first digest carries the unsubscribe leg VISIBLY ----------
    // (The render-side first-digest notice is asserted in render_fence;
    // here the marker it keys on is present at subscribe time.)
    assert_eq!(meta["auto_subscribed"], serde_json::json!(true));

    db.dispose().await;
}

/// The module surface the host registers: `user_created_handler()`
/// carries the builder's default digest; with none configured the
/// handler is still constructible (every event is an audited refusal).
#[tokio::test]
async fn module_builder_arms_the_loop_through_with_default_digest() {
    let db = TestDb::new("arm").await;
    let pool = db.pool.clone();

    let module = Arc::new(
        backbone_digest::DigestModule::builder()
            .with_database(pool.clone())
            .with_token_secret("probe-module-secret")
            .with_public_base_url("http://module.probe.test")
            .with_recipient_port(Arc::new(
                backbone_digest::application::service::engagement_port::SqlRecipientContext::new(pool.clone()),
            ))
            .build()
            .expect("module builds"),
    );
    // Base KPIs are registered by the builder.
    assert!(module.kpi_registry().get(backbone_digest::application::service::kpi_registry::KPI_CONNECTED_USERS).is_some());
    assert!(module.kpi_registry().get(backbone_digest::application::service::kpi_registry::KPI_MESSAGES_SENT).is_some());

    let co = seed_org_unit(&pool, "ARMED", "Armed Co").await;
    let now = Utc::now();
    let d = module
        .write_service()
        .create_digest("Armed Digest", backbone_digest::application::service::DigestPeriodicity::Weekly, now.date_naive())
        .await
        .expect("create");

    // The unarmed module's handler refuses (audited) —
    let unarmed = backbone_digest::DigestModule::builder()
        .with_database(pool.clone())
        .with_token_secret("probe-module-secret")
        .build()
        .expect("unarmed module builds");
    let u = seed_user(&pool, "armee@co.test", None).await;
    seed_membership(&pool, co, u).await;
    unarmed
        .user_created_handler()
        .handle(user_created_envelope(u))
        .await
        .expect("unarmed = audited refusal");
    let rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM digest.digest_subscriptions WHERE user_id = $1")
            .bind(u)
            .fetch_one(&pool)
            .await
            .expect("rows");
    assert_eq!(rows, 0);

    // ...and the public routes compose bare (the token is the auth).
    let router = module.digest_public_routes();
    let _ = router;

    // The armed module (rebuilt with the digest) subscribes through the
    // same surface the host registers.
    let armed = backbone_digest::DigestModule::builder()
        .with_database(pool.clone())
        .with_token_secret("probe-module-secret")
        .with_default_digest(d)
        .with_recipient_port(Arc::new(
            backbone_digest::application::service::engagement_port::SqlRecipientContext::new(pool.clone()),
        ))
        .build()
        .expect("armed module builds");
    armed
        .user_created_handler()
        .handle(user_created_envelope(u))
        .await
        .expect("armed handler subscribes");
    assert_eq!(
        super::common::subscription_state(&pool, d, u).await,
        "subscribed",
        "the armed loop subscribed the internal user"
    );

    db.dispose().await;
}
