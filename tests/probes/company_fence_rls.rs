//! Cross-company invisibility at the STORAGE layer: digest_digests is
//! FORCE row-level-security fenced on company_id; a non-superuser
//! session sees exactly its own `app.company_id` fence and NOTHING
//! (not an error, zero rows) without one. The sweep's cron-pool
//! posture (owner-role connection) is the documented operational
//! complement, not a policy hole here.

use chrono::Utc;
use uuid::Uuid;

use backbone_digest::application::service::DigestPeriodicity;

use super::common::{Svc, TestDb};

/// A cluster-scoped probe role with a panic-safe cleanup guard (a
/// leftover NOLOGIN role would outlive the scratch database).
struct ProbeRole {
    name: String,
    admin: sqlx::PgPool,
}

impl ProbeRole {
    async fn create(admin: &sqlx::PgPool) -> Self {
        let name = format!("digest_probe_fence_{}", Uuid::new_v4().simple());
        sqlx::query(&format!(r#"CREATE ROLE "{name}" NOLOGIN"#))
            .execute(admin)
            .await
            .expect("create probe role");
        sqlx::query(&format!(r#"GRANT USAGE ON SCHEMA digest TO "{name}""#))
            .execute(admin)
            .await
            .expect("grant schema usage");
        sqlx::query(&format!(r#"GRANT SELECT ON digest.digest_digests TO "{name}""#))
            .execute(admin)
            .await
            .expect("grant table select");
        Self { name, admin: admin.clone() }
    }
}

impl Drop for ProbeRole {
    fn drop(&mut self) {
        let name = self.name.clone();
        let url = std::env::var("DIGEST_TEST_ADMIN_URL")
            .unwrap_or_else(|_| super::common::SCRATCH_ADMIN_URL.to_string());
        std::thread::spawn(move || {
            if let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() {
                rt.block_on(async move {
                    if let Ok(admin) = sqlx::PgPool::connect(&url).await {
                        // DROP ROLE fails while privileges remain — drop
                        // the grants first, best-effort either way.
                        let _ = sqlx::query(&format!(
                            r#"REVOKE ALL ON digest.digest_digests FROM "{name}""#
                        ))
                        .execute(&admin)
                        .await;
                        let _ = sqlx::query(&format!(
                            r#"REVOKE ALL ON SCHEMA digest FROM "{name}""#
                        ))
                        .execute(&admin)
                        .await;
                        let _ = sqlx::query(&format!(r#"DROP ROLE IF EXISTS "{name}""#))
                            .execute(&admin)
                            .await;
                    }
                });
            }
        });
    }
}

#[tokio::test]
async fn the_company_fence_hides_other_companies_rows() {
    let db = TestDb::new("rls").await;
    let svc = Svc::new(db.pool.clone());
    let now = Utc::now();

    let co_a = Uuid::new_v4();
    let co_b = Uuid::new_v4();
    let co_c = Uuid::new_v4();
    // Two seeded digests (the superuser test pool writes freely) + one
    // DEACTIVATED digest still carrying company_a.
    let d_a = svc
        .write
        .create_digest("A Digest", co_a, DigestPeriodicity::Daily, now.date_naive())
        .await
        .expect("create A");
    let d_b = svc
        .write
        .create_digest("B Digest", co_b, DigestPeriodicity::Daily, now.date_naive())
        .await
        .expect("create B");
    let d_off = svc
        .write
        .create_digest("A Off Digest", co_a, DigestPeriodicity::Daily, now.date_naive())
        .await
        .expect("create A-off");
    svc.write.set_state(d_off, false).await.expect("deactivate");

    let role = ProbeRole::create(&db.pool).await;

    // Everything fenced happens inside ONE transaction: SET LOCAL ROLE
    // + set_config(..., true) both revert at commit/rollback, so
    // nothing leaks back into the pool.
    let mut tx = db.pool.begin().await.expect("begin");

    sqlx::query(&format!(r#"SET LOCAL ROLE "{}""#, role.name))
        .execute(&mut *tx)
        .await
        .expect("set role");

    // Fence = company A: sees ONLY company A's rows (both states).
    sqlx::query("SELECT set_config('app.company_id', $1, true)")
        .bind(co_a.to_string())
        .execute(&mut *tx)
        .await
        .expect("set fence A");
    let (n_a,): (i64,) = sqlx::query_as("SELECT count(*) FROM digest.digest_digests")
        .fetch_one(&mut *tx)
        .await
        .expect("count under fence A");
    assert_eq!(n_a, 2, "company A sees its two digests (activated + deactivated)");
    let ids: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM digest.digest_digests ORDER BY id")
        .fetch_all(&mut *tx)
        .await
        .expect("ids under fence A");
    assert!(ids.contains(&d_a) && ids.contains(&d_off), "exactly A's rows: {ids:?}");
    assert!(!ids.contains(&d_b), "the B row must be invisible under A's fence");

    // Fence = company B: only B's row.
    sqlx::query("SELECT set_config('app.company_id', $1, true)")
        .bind(co_b.to_string())
        .execute(&mut *tx)
        .await
        .expect("set fence B");
    let (n_b,): (i64,) = sqlx::query_as("SELECT count(*) FROM digest.digest_digests")
        .fetch_one(&mut *tx)
        .await
        .expect("count under fence B");
    assert_eq!(n_b, 1, "company B sees exactly its own digest");

    // NO fence at all (empty string): zero rows, NOT an error — this is
    // exactly why the KPI drop must never key on row count.
    sqlx::query("SELECT set_config('app.company_id', '', true)")
        .execute(&mut *tx)
        .await
        .expect("clear fence");
    let (n_none,): (i64,) = sqlx::query_as("SELECT count(*) FROM digest.digest_digests")
        .fetch_one(&mut *tx)
        .await
        .expect("count with no fence");
    assert_eq!(n_none, 0, "an unset fence reads zero rows (indistinguishable from empty)");

    // An unknown company's fence: also zero.
    sqlx::query("SELECT set_config('app.company_id', $1, true)")
        .bind(co_c.to_string())
        .execute(&mut *tx)
        .await
        .expect("set fence C");
    let (n_c,): (i64,) = sqlx::query_as("SELECT count(*) FROM digest.digest_digests")
        .fetch_one(&mut *tx)
        .await
        .expect("count under fence C");
    assert_eq!(n_c, 0);

    // Rollback reverts the role AND the fence (pool hygiene).
    tx.rollback().await.expect("rollback");

    db.dispose().await;
    drop(role);
}
