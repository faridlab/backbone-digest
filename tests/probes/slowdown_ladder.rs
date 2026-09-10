//! The slowdown ladder + the daily pull sweep (cron-only degradation,
//! quarterly floor, mail-failure no-advance, per-digest isolation,
//! manual Send Now never touching the ladder or the schedule).

use std::sync::Arc;

use chrono::{Duration, Utc};
use uuid::Uuid;

use backbone_digest::application::service::digest_error::DigestError;
use backbone_digest::application::service::digest_mail_seam::{
    DigestMailSeam, OutgoingDigestMail, RecordingMailSeam,
};
use backbone_digest::application::service::digest_render_service::DigestRenderService;
use backbone_digest::application::service::digest_cron_service::DigestCronService;
use backbone_digest::application::service::DigestPeriodicity;

use super::common::{digest_cadence, seed_membership, seed_org_unit, seed_user, Svc, TestDb};

/// A seam that refuses exactly ONE recipient's mail and records
/// everything else through the shared recording double (the
/// mail-delivery-failure seat needs per-recipient refusal).
struct FailForUser {
    user: Uuid,
    inner: Arc<RecordingMailSeam>,
}

#[async_trait::async_trait]
impl DigestMailSeam for FailForUser {
    async fn send_digest_mail(&self, mail: &OutgoingDigestMail) -> Result<Uuid, DigestError> {
        if mail.recipient_user_id == self.user {
            Err(DigestError::MailDelivery(
                "probe: the smtp leg refuses this one recipient".into(),
            ))
        } else {
            self.inner.send_digest_mail(mail).await
        }
    }
}

/// One sweep over five due digests covering every ladder rule.
#[tokio::test]
async fn the_ladder_and_the_advance_rules() {
    let db = TestDb::new("ladder").await;
    let svc = Svc::new(db.pool.clone());
    svc.install_sql_port();
    let now = Utc::now();
    let today = now.date_naive();
    let due_since = today - Duration::days(7);
    let co = seed_org_unit(&db.pool, "LADDER", "Ladder Co").await;

    // d1: DAILY, recipient NEVER logged in → degrade to weekly, advance
    //     by the NEW cadence, mail still sent.
    let d1 = svc
        .write
        .create_digest("Ladder Daily", DigestPeriodicity::Daily, due_since)
        .await
        .expect("create d1");
    let u1 = seed_user(&db.pool, "cold@x.test", None).await;
    seed_membership(&db.pool, co, u1).await;
    assert!(svc.write.subscribe_user(d1, u1, None).await.expect("sub u1"));

    // d2: DAILY, recipient logged in an hour ago → NO degradation,
    //     advance by daily.
    let d2 = svc
        .write
        .create_digest("Engaged Daily", DigestPeriodicity::Daily, due_since)
        .await
        .expect("create d2");
    let u2 = seed_user(&db.pool, "warm@x.test", Some(now - Duration::hours(1))).await;
    seed_membership(&db.pool, co, u2).await;
    assert!(svc.write.subscribe_user(d2, u2, None).await.expect("sub u2"));

    // d3: QUARTERLY, no login → the floor holds (no fifth rung). The
    //     create anchor is 100 days back so the quarterly advance from
    //     it lands in the past (a quarterly digest created "today" is
    //     next due in three months and would never enter this sweep).
    let d3 = svc
        .write
        .create_digest(
            "Floor Quarterly",
            DigestPeriodicity::Quarterly,
            today - Duration::days(100),
        )
        .await
        .expect("create d3");
    let u3 = seed_user(&db.pool, "q@x.test", None).await;
    seed_membership(&db.pool, co, u3).await;
    assert!(svc.write.subscribe_user(d3, u3, None).await.expect("sub u3"));

    // d4: DAILY, ENGAGED recipient, but the smtp leg refuses exactly
    //     them → cadence untouched AND deliberately UNADVANCED (retry
    //     whole next day).
    let d4 = svc
        .write
        .create_digest("Smtp Down", DigestPeriodicity::Daily, due_since)
        .await
        .expect("create d4");
    let u4 = seed_user(&db.pool, "smtp@x.test", Some(now - Duration::hours(1))).await;
    seed_membership(&db.pool, co, u4).await;
    assert!(svc.write.subscribe_user(d4, u4, None).await.expect("sub u4"));

    // d5: DAILY, no login — but the MANUAL send runs first: Send Now
    //     must not degrade it nor advance it.
    let d5 = svc
        .write
        .create_digest("Manual Only", DigestPeriodicity::Daily, due_since)
        .await
        .expect("create d5");
    let u5 = seed_user(&db.pool, "manual@x.test", None).await;
    seed_membership(&db.pool, co, u5).await;
    assert!(svc.write.subscribe_user(d5, u5, None).await.expect("sub u5"));

    // ---- manual Send Now FIRST (d5): never degrades, never advances ----
    let (sent, mail_fail, unexpected) = svc.cron.send_now(d5, now).await.expect("send_now");
    assert_eq!((sent, mail_fail, unexpected), (1, 0, 0), "the manual send delivers once");
    let (p5, n5) = digest_cadence(&db.pool, d5).await;
    assert_eq!(p5, "daily", "Send Now never degrades the cadence");
    assert_eq!(
        n5,
        Some(due_since + Duration::days(1)),
        "Send Now never advances next_run_date (the digest stays due)"
    );
    assert_eq!(svc.seam.sent().len(), 1, "exactly the one manual mail so far");

    // ---- the daily pull over everything due -----------------------------
    // The sweep runs on a seam that refuses ONLY u4's mail; everything
    // else flows through the shared recording double.
    let selective = Arc::new(FailForUser { user: u4, inner: svc.seam.clone() });
    let render = Arc::new(DigestRenderService::new(
        db.pool.clone(),
        svc.registry.clone(),
        svc.slot.clone(),
        selective,
        Some(svc.unsub.clone()),
        "http://digest.probe.test",
    ));
    let cron = DigestCronService::new(db.pool.clone(), render, svc.slot.clone());
    let outcome = cron.run_daily_pull(now).await.expect("daily pull");

    // Every due digest was claimed exactly once (d1..d5 — d5 too: Send
    // Now left it due, and the cron path degrades it legitimately).
    assert_eq!(outcome.claimed, 5, "claimed: {} (d1-d5 all due)", outcome.claimed);
    assert!(outcome.isolated_failures.is_empty(), "no digest hit an unexpected failure");

    // d1 degraded one rung to WEEKLY and advanced by the weekly span.
    assert!(outcome.degraded.contains(&d1), "the cold daily digest degrades");
    let (p1, n1) = digest_cadence(&db.pool, d1).await;
    assert_eq!(p1, "weekly");
    assert_eq!(n1, Some(DigestPeriodicity::Weekly.advance(today)));

    // d2 engaged: no degradation, daily advance.
    assert!(!outcome.degraded.contains(&d2));
    let (p2, n2) = digest_cadence(&db.pool, d2).await;
    assert_eq!(p2, "daily");
    assert_eq!(n2, Some(DigestPeriodicity::Daily.advance(today)));

    // d3: the quarterly floor — no rung below, stays quarterly,
    //     still advances.
    assert!(!outcome.degraded.contains(&d3), "quarterly is the floor");
    let (p3, n3) = digest_cadence(&db.pool, d3).await;
    assert_eq!(p3, "quarterly");
    assert_eq!(n3, Some(DigestPeriodicity::Quarterly.advance(today)));

    // d4: the smtp leg refused its one recipient → cadence held,
    //     UNADVANCED (still due).
    assert!(outcome.mail_delivery_failures.contains(&d4));
    assert!(!outcome.degraded.contains(&d4), "u4 is engaged; no degradation before the refusal");
    let (p4, n4) = digest_cadence(&db.pool, d4).await;
    assert_eq!(p4, "daily");
    assert_eq!(
        n4,
        Some(due_since + Duration::days(1)),
        "a mail-delivery failure deliberately leaves the digest due"
    );

    // d5 in the sweep: no login signal → degraded now (cron path),
    // advanced by the new weekly cadence.
    assert!(outcome.degraded.contains(&d5), "the sweep degrades the never-engaged digest");
    let (p5b, n5b) = digest_cadence(&db.pool, d5).await;
    assert_eq!(p5b, "weekly");
    assert_eq!(n5b, Some(DigestPeriodicity::Weekly.advance(today)));

    // Mails: d1, d2, d3, d5 sent by the sweep (u4's refused), plus d5's
    // earlier manual send.
    assert_eq!(svc.seam.sent().len(), 5, "4 sweep mails + 1 manual mail");
    assert_eq!(outcome.sent_mails, 4, "the sweep enqueued four mails");

    // A second pull, healthy seam: only the unadvanced d4 is still due;
    // it retries whole, sends, and advances.
    let outcome2 = svc.cron.run_daily_pull(now + Duration::hours(1)).await.expect("second pull");
    assert_eq!(outcome2.claimed, 1, "only the unadvanced d4 retries");
    assert!(!outcome2.degraded.contains(&d4));
    assert_eq!(outcome2.sent_mails, 1);
    let (_, n4b) = digest_cadence(&db.pool, d4).await;
    assert_eq!(
        n4b,
        Some(DigestPeriodicity::Daily.advance(today)),
        "d4 advances once its retry succeeds"
    );

    // A third pull is idle: the due-date column IS the queue.
    let outcome3 = svc.cron.run_daily_pull(now + Duration::hours(2)).await.expect("third pull");
    assert_eq!(outcome3.claimed, 0, "nothing left due today");

    db.dispose().await;
}

/// The sweep signal fails closed: an unwired port HOLDS the cadence
/// (never degrades on a guess). The recipients' renders also refuse
/// (identity is unresolvable), so the digest is isolated and left due
/// — the sweep CONTINUES (the decided delta vs upstream's
/// die-mid-loop).
#[tokio::test]
async fn an_unavailable_signal_holds_the_cadence() {
    let db = TestDb::new("signal").await;
    let svc = Svc::new(db.pool.clone());
    // NO port install: every identity fact (signal AND resolve) refuses.
    let now = Utc::now();

    let d = svc
        .write
        .create_digest(
            "Signal Digest",
            DigestPeriodicity::Daily,
            now.date_naive() - Duration::days(7),
        )
        .await
        .expect("create digest");
    let u = seed_user(&db.pool, "sig@x.test", None).await;
    assert!(svc.write.subscribe_user(d, u, None).await.expect("subscribe"));

    let outcome = svc.cron.run_daily_pull(now).await.expect("pull");
    assert!(!outcome.degraded.contains(&d), "a refused signal never degrades");
    assert!(
        outcome.isolated_failures.contains(&d),
        "the unrenderable digest is isolated, not fatal: {:?}",
        outcome
    );
    let (p, _n) = digest_cadence(&db.pool, d).await;
    assert_eq!(p, "daily", "the cadence is held on an unavailable signal");

    db.dispose().await;
}
