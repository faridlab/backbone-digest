//! The RFC 8058 one-click WIRE shape: the token rides the QUERY string
//! (the MUA POSTs to the URL the List-Unsubscribe header carries, with
//! the fixed body `List-Unsubscribe=One-Click` and no `t` field). This
//! probe drives the composed PUBLIC ROUTER — not the service — so a
//! handler that binds only the form body cannot pass it: the unsubscribe
//! would silently no-op while still answering the bare 200.

use chrono::{Duration, Utc};

use backbone_digest::application::service::DigestPeriodicity;

use super::common::{seed_user, subscription_state, Svc, TestDb, PROBE_SECRET};

/// The full module (armed unsubscribe engine) over a real scratch DB,
/// with its public router extracted for oneshot requests.
struct Wire {
    db: TestDb,
    svc: Svc,
    app: axum::Router,
}

impl Wire {
    async fn new() -> Self {
        let db = TestDb::new("wire").await;
        let module = std::sync::Arc::new(
            backbone_digest::DigestModule::builder()
                .with_database(db.pool.clone())
                // Same secret as the Svc harness so a token minted by
                // either side verifies on the other.
                .with_token_secret(PROBE_SECRET)
                .build()
                .expect("armed module build"),
        );
        let svc = Svc::new(db.pool.clone());
        let app = module.digest_public_routes();
        Self { db, svc, app }
    }
}

/// One RFC 8058 POST: token in the QUERY, the fixed marker in the body.
async fn one_click(app: &axum::Router, token: &str) -> axum::http::StatusCode {
    use axum::body::Body;
    use tower::ServiceExt;
    app.clone()
        .oneshot(
            axum::http::Request::post(format!(
                "/digest/unsubscribe?t={token}"
            ))
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from("List-Unsubscribe=One-Click"))
            .unwrap(),
        )
        .await
        .expect("oneshot")
        .status()
}

/// The genuine MUA wire shape performs the unsubscribe: token in the
/// query, `List-Unsubscribe=One-Click` in the body — the tombstone
/// lands, the re-POST is the idempotent no-op, and the refused shapes
/// (expired, forged, malformed — all in the query) perform nothing.
/// Every answer on this leg is the bare 200 (no oracle).
#[tokio::test]
async fn one_click_wire_shape_query_token_performs_and_refuses() {
    let Wire { db, svc, app } = Wire::new().await;
    let now = Utc::now();

    let digest = svc
        .write
        .create_digest("Wire Digest", DigestPeriodicity::Daily, now.date_naive())
        .await
        .expect("create digest");
    let user = seed_user(&db.pool, "wire@x.test", None).await;
    assert!(svc.write.subscribe_user(digest, user, None).await.expect("subscribe"));

    // ---- a real token, carried in the QUERY, performs -----------------
    let token = svc.unsub.mint_link_token(digest, user);
    assert_eq!(one_click(&app, &token).await, axum::http::StatusCode::OK);
    assert_eq!(
        subscription_state(&db.pool, digest, user).await,
        "unsubscribed",
        "the query-carried token on the real wire shape must land the tombstone"
    );

    // ---- the SAME token re-POSTed: idempotent no-op, still 200 --------
    assert_eq!(one_click(&app, &token).await, axum::http::StatusCode::OK);
    assert_eq!(subscription_state(&db.pool, digest, user).await, "unsubscribed");

    // ---- resubscribe, then the refused shapes (all in the query) ------
    assert!(svc.unsub.resubscribe(digest, user).await.expect("resubscribe"));
    assert_eq!(subscription_state(&db.pool, digest, user).await, "subscribed");

    // EXPIRED: minted 200 days ago (the 180-day TTL has passed).
    let stale = svc.unsub.mint(digest, user, (now - Duration::days(200)).naive_utc());
    let stale_str = format!("{}.{}.{}.{}", stale.digest_id, stale.user_id, stale.exp, stale.mac);
    assert_eq!(one_click(&app, &stale_str).await, axum::http::StatusCode::OK);
    assert_eq!(subscription_state(&db.pool, digest, user).await, "subscribed");

    // FORGED: one flipped MAC hex char.
    let minted = svc.unsub.mint_link_token(digest, user);
    let flipped = {
        let mut chars = minted.chars().collect::<Vec<_>>();
        let last = chars.last_mut().expect("nonempty");
        *last = if *last == 'a' { 'b' } else { 'a' };
        chars.into_iter().collect::<String>()
    };
    assert_eq!(one_click(&app, &flipped).await, axum::http::StatusCode::OK);
    assert_eq!(subscription_state(&db.pool, digest, user).await, "subscribed");

    // MALFORMED: garbage performs nothing.
    assert_eq!(one_click(&app, "garbage-token").await, axum::http::StatusCode::OK);
    assert_eq!(subscription_state(&db.pool, digest, user).await, "subscribed");

    // ---- NO token anywhere (query and body both empty): noise, 200 ----
    use axum::body::Body;
    use tower::ServiceExt;
    let resp = app
        .clone()
        .oneshot(
            axum::http::Request::post("/digest/unsubscribe")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("List-Unsubscribe=One-Click"))
                .unwrap(),
        )
        .await
        .expect("oneshot");
    assert_eq!(resp.status(), axum::http::StatusCode::OK);
    assert_eq!(subscription_state(&db.pool, digest, user).await, "subscribed");

    db.dispose().await;
}

/// The form-body carrier still works when it carries the token (manual
/// posts, curl probes): the resolution order is form.then(query).
#[tokio::test]
async fn one_click_form_body_carrier_still_applies() {
    let Wire { db, svc, app } = Wire::new().await;
    let now = Utc::now();

    let digest = svc
        .write
        .create_digest("Form Digest", DigestPeriodicity::Weekly, now.date_naive())
        .await
        .expect("create digest");
    let user = seed_user(&db.pool, "form@x.test", None).await;
    assert!(svc.write.subscribe_user(digest, user, None).await.expect("subscribe"));

    use axum::body::Body;
    use tower::ServiceExt;
    let token = svc.unsub.mint_link_token(digest, user);
    let resp = app
        .oneshot(
            axum::http::Request::post("/digest/unsubscribe")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(format!("t={token}")))
                .unwrap(),
        )
        .await
        .expect("oneshot");
    assert_eq!(resp.status(), axum::http::StatusCode::OK);
    assert_eq!(
        subscription_state(&db.pool, digest, user).await,
        "unsubscribed",
        "a form-body token must still perform the unsubscribe"
    );

    db.dispose().await;
}
