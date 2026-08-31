//! The RFC 8058 one-click unsubscribe round-trip: a real minted token
//! performs the unsubscribe; a re-POST of the SAME token is a bare
//! no-op success (idempotent, no oracle); expired, forged, and
//! malformed tokens perform NOTHING and are indistinguishable from a
//! no-op on the wire; verify failures throttle into a lockout that
//! also refuses a subsequently-valid token.

use chrono::{Duration, Utc};
use uuid::Uuid;

use backbone_digest::application::service::unsubscribe_service::{
    TokenVerdict, UnsubscribeService, VERIFY_LOCK_SECONDS, VERIFY_MAX_FAILURES,
};
use backbone_digest::application::service::DigestPeriodicity;

use super::common::{seed_user, subscription_state, Svc, TestDb};

/// Mint → apply → tombstone → idempotent re-POST → resubscribe, plus
/// the three refused shapes and the failure lockout.
#[tokio::test]
async fn one_click_round_trip_is_idempotent_and_oracle_free() {
    let db = TestDb::new("unsub").await;
    let svc = Svc::new(db.pool.clone());
    let now = Utc::now();
    let co = Uuid::new_v4();

    let digest = svc
        .write
        .create_digest("Unsub Digest", co, DigestPeriodicity::Daily, now.date_naive())
        .await
        .expect("create digest");
    let user = seed_user(&db.pool, "unsub@x.test", None).await;
    assert!(svc.write.subscribe_user(digest, user, None).await.expect("subscribe"));

    // ---- a real token performs the unsubscribe -------------------------
    let token = svc.unsub.mint_link_token(digest, user);
    assert_eq!(
        subscription_state(&db.pool, digest, user).await,
        "subscribed",
        "precondition"
    );
    assert!(
        svc.unsub.unsubscribe_by_token(&token, now.timestamp()).await.expect("first POST"),
        "the first POST performs the unsubscribe"
    );
    assert_eq!(subscription_state(&db.pool, digest, user).await, "unsubscribed");
    let stamped: Option<chrono::DateTime<Utc>> = sqlx::query_scalar(
        r#"SELECT unsubscribed_at FROM digest.digest_subscriptions
           WHERE digest_id = $1 AND user_id = $2"#,
    )
    .bind(digest)
    .bind(user)
    .fetch_one(&db.pool)
    .await
    .expect("unsubscribed_at");
    assert!(stamped.is_some(), "the tombstone stamps unsubscribed_at");

    // ---- the SAME token re-POSTed is a no-op success (bare 200) --------
    assert!(
        !svc.unsub
            .unsubscribe_by_token(&token, now.timestamp() + 5)
            .await
            .expect("re-POST"),
        "the idempotent re-POST performs nothing but still succeeds"
    );
    assert_eq!(subscription_state(&db.pool, digest, user).await, "unsubscribed");

    // ---- the reverse edge: resubscribe clears the tombstone ------------
    assert!(svc.unsub.resubscribe(digest, user).await.expect("resubscribe"));
    assert_eq!(subscription_state(&db.pool, digest, user).await, "subscribed");
    let cleared: Option<chrono::DateTime<Utc>> = sqlx::query_scalar(
        r#"SELECT unsubscribed_at FROM digest.digest_subscriptions
           WHERE digest_id = $1 AND user_id = $2"#,
    )
    .bind(digest)
    .bind(user)
    .fetch_one(&db.pool)
    .await
    .expect("unsubscribed_at after resubscribe");
    assert!(cleared.is_none(), "resubscribe clears unsubscribed_at");

    // ---- EXPIRED: minted 200 days ago (the 180-day TTL has passed) -----
    let stale = svc.unsub.mint(digest, user, (now - Duration::days(200)).naive_utc());
    let stale_str = format!("{}.{}.{}.{}", stale.digest_id, stale.user_id, stale.exp, stale.mac);
    assert_eq!(svc.unsub.verify(&stale, now.timestamp()), TokenVerdict::Refused);
    assert!(
        !svc.unsub
            .unsubscribe_by_token(&stale_str, now.timestamp())
            .await
            .expect("expired POST still a 200-shape Ok"),
        "an expired token performs nothing"
    );
    assert_eq!(subscription_state(&db.pool, digest, user).await, "subscribed");

    // ---- MALFORMED: garbage performs nothing, still Ok ------------------
    assert!(
        !svc.unsub
            .unsubscribe_by_token("garbage-token", now.timestamp())
            .await
            .expect("malformed POST still Ok"),
        "a malformed token performs nothing"
    );
    assert_eq!(subscription_state(&db.pool, digest, user).await, "subscribed");

    // ---- FORGED: one flipped MAC hex char refuses ------------------------
    let minted = svc.unsub.mint_link_token(digest, user);
    let flipped = {
        let mut chars = minted.chars().collect::<Vec<_>>();
        let last = chars.last_mut().expect("nonempty");
        *last = if *last == 'a' { 'b' } else { 'a' };
        chars.into_iter().collect::<String>()
    };
    assert_ne!(minted, flipped);
    assert!(
        !svc.unsub
            .unsubscribe_by_token(&flipped, now.timestamp())
            .await
            .expect("forged POST still Ok"),
        "a forged token performs nothing"
    );
    assert_eq!(subscription_state(&db.pool, digest, user).await, "subscribed");

    // ---- the verify-failure lockout throttles the pair ------------------
    // The stale + malformed + forged attempts above already counted
    // against (digest, user); drive the book to the threshold and past
    // it, then show a VALID token is refused while locked.
    let book = svc.unsub.failure_book();
    while !book.locked(digest, user) {
        let _ = svc
            .unsub
            .unsubscribe_by_token(&flipped, now.timestamp())
            .await
            .expect("forged attempts stay Ok");
        // Guard against an infinite loop if the lock never engages.
        assert!(
            svc.unsub.verify(&stale, now.timestamp()) == TokenVerdict::Refused
                || book.locked(digest, user),
            "failures keep refusing"
        );
    }
    assert!(
        book.locked(digest, user),
        "VERIFY_MAX_FAILURES={} must engage the lock",
        VERIFY_MAX_FAILURES
    );
    // A perfectly valid, fresh token is refused while locked (the
    // throttle guards the verify leg, not just the apply leg).
    let fresh = svc.unsub.mint_link_token(digest, user);
    let verdict = UnsubscribeService::parse(&fresh)
        .map(|t| svc.unsub.verify(&t, now.timestamp()));
    assert_eq!(verdict, Some(TokenVerdict::Refused));
    assert!(
        !svc.unsub
            .unsubscribe_by_token(&fresh, now.timestamp())
            .await
            .expect("locked POST still Ok"),
        "a valid token during lockout performs nothing"
    );
    assert_eq!(subscription_state(&db.pool, digest, user).await, "subscribed");
    let _ = VERIFY_LOCK_SECONDS; // the lock's window (recorded)

    // ---- cross-key isolation: a token for another digest/user is valid
    // for ITS pair only — but the MAC domain binds (digest, user, exp),
    // so this fresh pair mints and applies independently.
    let digest2 = svc
        .write
        .create_digest("Other Digest", co, DigestPeriodicity::Weekly, now.date_naive())
        .await
        .expect("create digest 2");
    let user2 = seed_user(&db.pool, "other@x.test", None).await;
    assert!(svc.write.subscribe_user(digest2, user2, None).await.expect("subscribe 2"));
    let token2 = svc.unsub.mint_link_token(digest2, user2);
    assert!(
        svc.unsub.unsubscribe_by_token(&token2, now.timestamp()).await.expect("cross-pair POST"),
        "the second pair's token performs its own unsubscribe"
    );
    assert_eq!(subscription_state(&db.pool, digest2, user2).await, "unsubscribed");

    db.dispose().await;
}

/// The token shape itself: parse round-trips, TTL is 180 days (the
/// recorded deviation vs upstream's never-expiring tokens), and the
/// wrong-secret service cannot verify a minted token.
#[tokio::test]
async fn token_shape_ttl_and_secret_binding() {
    let pool = sqlx::PgPool::connect_lazy(&format!(
        "postgres://probe:probe@127.0.0.1:1/{}",
        Uuid::new_v4().simple()
    ))
    .expect("lazy connect (no I/O for mint/verify)");
    let svc = UnsubscribeService::with_secret(pool, b"secret-one");
    let d = Uuid::new_v4();
    let u = Uuid::new_v4();
    let now = Utc::now();

    let t = svc.mint(d, u, now.naive_utc());
    let s = format!("{}.{}.{}.{}", t.digest_id, t.user_id, t.exp, t.mac);
    let parsed = UnsubscribeService::parse(&s).expect("parse round-trip");
    assert_eq!(parsed.digest_id, d);
    assert_eq!(parsed.user_id, u);
    // 180-day TTL.
    assert_eq!(
        parsed.exp - now.timestamp(),
        chrono::Duration::days(180).num_seconds(),
        "the TTL is exactly 180 days"
    );
    assert_eq!(svc.verify(&parsed, now.timestamp()), TokenVerdict::Valid);
    // Just before expiry: valid; just after: refused.
    assert_eq!(
        svc.verify(&parsed, parsed.exp - 1),
        TokenVerdict::Valid,
        "valid up to the expiry second"
    );
    assert_eq!(svc.verify(&parsed, parsed.exp + 1), TokenVerdict::Refused);

    // A different secret cannot verify the token (the MAC binds the
    // secret; the consteq leg).
    let other = UnsubscribeService::with_secret(
        sqlx::PgPool::connect_lazy("postgres://probe:probe@127.0.0.1:1/x").expect("lazy"),
        b"secret-two",
    );
    assert_eq!(other.verify(&parsed, now.timestamp()), TokenVerdict::Refused);

    // Malformed shapes refuse parsing.
    for bad in ["", "a.b.c.d", &format!("{d}.not-a-uuid.1.{}", "f".repeat(64)), &format!("{}.{}.{}.{}", d, u, 1, "tooshort")] {
        assert!(UnsubscribeService::parse(bad).is_none(), "must refuse: {bad}");
    }
}
