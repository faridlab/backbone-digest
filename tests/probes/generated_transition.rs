//! The generated transition endpoints move a state field that generic
//! writes may not touch. `state` is both write-protected (hand-set) and
//! bound to a state machine; the generated verb writes it through the
//! generic partial update inside `as_transition`, so the entity must name it
//! among its state fields or every transition is refused as
//! `field_not_writable`. This probe drives the generated transition router
//! end to end and reads the stored state back.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use backbone_auth::middleware::AuthContext;
use chrono::Utc;
use tower::ServiceExt;

use backbone_digest::application::service::digest_digest_service::DigestDigestRowRules;
use backbone_digest::application::service::DigestPeriodicity;
use backbone_digest::presentation::http::digest_digest_handler::create_digest_digest_transition_routes;
use backbone_digest::{DigestDigestRepository, DigestDigestService};

use super::common::{Svc, TestDb};

async fn digest_state(pool: &sqlx::PgPool, id: uuid::Uuid) -> String {
    sqlx::query_scalar::<_, String>(
        r#"SELECT state::text FROM digest.digest_digests WHERE id = $1"#,
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .expect("digest row")
}

async fn post_transition(app: &axum::Router, id: uuid::Uuid, verb: &str) -> StatusCode {
    let mut auth = AuthContext::new("probe-user".into());
    auth.permissions = vec!["digest_digest:transition:*".into()];
    let mut req = Request::post(format!("/digest_digests/{id}/transitions/{verb}"))
        .body(Body::empty())
        .unwrap();
    req.extensions_mut().insert(auth);
    app.clone().oneshot(req).await.expect("oneshot").status()
}

/// Deactivate then activate an activated digest through the generated
/// routes: each answers 200 and moves the stored state.
#[tokio::test]
async fn generated_transitions_move_the_protected_state_field() {
    let db = TestDb::new("transition").await;
    let svc = Svc::new(db.pool.clone());
    let digest = svc
        .write
        .create_digest(
            "Transition Digest",
            DigestPeriodicity::Daily,
            Utc::now().date_naive(),
        )
        .await
        .expect("create digest");

    let service = Arc::new(
        DigestDigestService::with_repository(Arc::new(DigestDigestRepository::new(
            db.pool.clone(),
        )))
        .with_guard(Arc::new(DigestDigestRowRules)),
    );
    let app = create_digest_digest_transition_routes(service);

    assert_eq!(
        post_transition(&app, digest, "action_deactivate").await,
        StatusCode::OK
    );
    assert_eq!(digest_state(&db.pool, digest).await, "deactivated");

    assert_eq!(
        post_transition(&app, digest, "action_activate").await,
        StatusCode::OK
    );
    assert_eq!(digest_state(&db.pool, digest).await, "activated");

    db.dispose().await;
}
