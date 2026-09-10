//! Shared harness: one DISPOSABLE scratch database per test, FAIL-HARD.
//!
//! The suite never runs against a shared DB: each test mints
//! `digest_seat_<marker>_<hex>` on the local scratch Postgres (5433,
//! postgres/postgres), applies this module's migrations plus the
//! sibling sets its SQL adapters read (the organization spine sapiens'
//! membership chain builds on, sapiens identity, messaging mail rows)
//! and the outbox schema mail's enqueue stages into, runs, and drops
//! the database.
//!
//! FAIL-HARD CONTRACT (the survey/mailing harness contract, carried
//! over): a test that cannot reach its scratch database PANICS —
//! [`TestDb::new`] refuses to return `None`, and [`skipped`] panics on
//! principle. A skipped probe is a FAILED probe: a green suite means
//! the behaviors were exercised, not that they were unreachable.
//!
//! `TestDb::dispose()` is the explicit teardown; `Drop` is the leak
//! guard (best-effort DROP on a throwaway runtime) for panicking tests.

use std::sync::Arc;
use std::time::Duration;

use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use uuid::Uuid;

use backbone_digest::application::service::base_kpis::register_base_kpis;
use backbone_digest::application::service::digest_cron_service::DigestCronService;
use backbone_digest::application::service::digest_mail_seam::RecordingMailSeam;
use backbone_digest::application::service::digest_render_service::DigestRenderService;
use backbone_digest::application::service::digest_write_service::DigestWriteService;
use backbone_digest::application::service::engagement_port::{
    RecipientContextSlot, SqlRecipientContext,
};
use backbone_digest::application::service::kpi_registry::KpiRegistry;
use backbone_digest::application::service::unsubscribe_service::UnsubscribeService;

/// The scratch Postgres every test database is born on and dropped from.
pub const SCRATCH_ADMIN_URL: &str = "postgres://postgres:postgres@localhost:5433/postgres";

/// The HMAC secret the probe services share (explicit, never from the
/// environment — the probes must not depend on host configuration).
pub const PROBE_SECRET: &[u8] = b"digest-probe-secret";

fn admin_url() -> String {
    std::env::var("DIGEST_TEST_ADMIN_URL").unwrap_or_else(|_| SCRATCH_ADMIN_URL.into())
}

/// The fail-hard skip: reaching this is a FAILURE, never a green tick.
pub fn skipped(reason: &str) -> ! {
    panic!("VACUOUS SKIP IS A FAILURE: {reason}");
}

/// One disposable scratch database, migrations applied. Panics (never
/// returns `None`) when the scratch Postgres is unreachable — see the
/// module docs for the fail-hard contract.
pub struct TestDb {
    pub pool: PgPool,
    name: String,
    admin: PgPool,
}

impl TestDb {
    pub async fn new(marker: &str) -> Self {
        let url = admin_url();
        let admin = match PgPoolOptions::new()
            .max_connections(4)
            .acquire_timeout(Duration::from_secs(5))
            .connect(&url)
            .await
        {
            Ok(a) => a,
            Err(e) => {
                eprintln!("PROBE-FAIL: {marker}: admin connect to {url} failed: {e}");
                skipped(&format!("scratch Postgres unreachable: {e}"));
            }
        };
        let suffix: String = Uuid::new_v4().simple().to_string().chars().take(8).collect();
        let name = format!("digest_seat_{marker}_{suffix}");
        // Disposable by construction: a stale DB of the same name goes first.
        if let Err(e) =
            sqlx::query(&format!(r#"DROP DATABASE IF EXISTS "{name}" WITH (FORCE)"#)).execute(&admin).await
        {
            eprintln!("PROBE-FAIL: {marker}: pre-drop of {name} failed: {e}");
            skipped(&format!("scratch pre-drop failed: {e}"));
        }
        if let Err(e) = sqlx::query(&format!(r#"CREATE DATABASE "{name}""#)).execute(&admin).await {
            eprintln!("PROBE-FAIL: {marker}: create database {name} failed: {e}");
            skipped(&format!("scratch create failed: {e}"));
        }
        // Splice ONLY the trailing path segment.
        let db_url = match url.rfind('/') {
            Some(i) => format!("{}{}", &url[..=i], name),
            None => url.clone(),
        };
        let pool = match PgPoolOptions::new()
            .max_connections(8)
            .acquire_timeout(Duration::from_secs(10))
            .connect(&db_url)
            .await
        {
            Ok(p) => p,
            Err(e) => {
                eprintln!("PROBE-FAIL: {marker}: connect to {db_url} failed: {e}");
                skipped(&format!("scratch connect failed: {e}"));
            }
        };
        if let Err(what) = apply_sibling_migrations(&pool, marker).await {
            skipped(&what);
        }
        Self { pool, name, admin }
    }

    /// The admin pool (scratch-cluster connections — the RLS probes
    /// mint their non-superuser roles through it).
    pub fn admin_pool(&self) -> &PgPool {
        &self.admin
    }

    /// Explicit teardown: drop the scratch database entirely.
    pub async fn dispose(self) {
        self.drop_db().await;
    }

    async fn drop_db(&self) {
        // FORCE: connected test pool may still hold an idle session.
        let _ = sqlx::query(&format!(r#"DROP DATABASE IF EXISTS "{}" WITH (FORCE)"#, self.name))
            .execute(&self.admin)
            .await;
    }
}

impl Drop for TestDb {
    fn drop(&mut self) {
        let name = self.name.clone();
        let url = admin_url();
        // Leak-guard teardown for panicking tests; dispose() is the happy path.
        std::thread::spawn(move || {
            if let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() {
                rt.block_on(async move {
                    if let Ok(admin) = sqlx::PgPool::connect(&url).await {
                        let _ = sqlx::query(&format!(
                            r#"DROP DATABASE IF EXISTS "{name}" WITH (FORCE)"#
                        ))
                        .execute(&admin)
                        .await;
                    }
                });
            }
        });
    }
}

/// Apply this module's migrations plus the sibling sets the module's
/// SQL adapters read (organization: the org-unit spine sapiens'
/// membership rekey chain requires, including the seeded tenant root;
/// sapiens: users/memberships/roles; mail: the messaging.mail_messages
/// volume the Messages Sent KPI counts), each set in its own sorted
/// batch — the order each module's own runner uses. The outbox schema
/// mail's enqueue stages into is migrated by the outbox crate itself
/// (the mailing-module precedent).
async fn apply_sibling_migrations(pool: &PgPool, marker: &str) -> Result<(), String> {
    let manifest = env!("CARGO_MANIFEST_DIR");
    let dirs = [
        format!("{manifest}/migrations"),
        format!("{manifest}/../backbone-organization/migrations"),
        format!("{manifest}/../backbone-sapiens/migrations"),
        format!("{manifest}/../backbone-mail/migrations"),
    ];
    for dir in dirs {
        let mut files: Vec<std::path::PathBuf> = match std::fs::read_dir(&dir) {
            Ok(rd) => rd
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name().and_then(|n| n.to_str()).map(|n| n.ends_with(".up.sql")).unwrap_or(false)
                })
                .collect(),
            Err(e) => return Err(format!("PROBE-FAIL: {marker}: cannot read {dir}: {e}")),
        };
        files.sort();
        let mut conn = pool
            .acquire()
            .await
            .map_err(|e| format!("PROBE-FAIL: {marker}: cannot acquire pool conn: {e}"))?;
        for file in files {
            let sql = std::fs::read_to_string(&file)
                .map_err(|e| format!("PROBE-FAIL: {marker}: cannot read {}: {e}", file.display()))?;
            if let Err(e) = sqlx::raw_sql(&sql).execute(&mut *conn).await {
                return Err(format!("PROBE-FAIL: {marker}: migration {} failed: {e}", file.display()));
            }
        }
    }
    backbone_outbox::outbox::migrate(pool, "messaging")
        .await
        .map_err(|e| format!("PROBE-FAIL: {marker}: outbox migrate (messaging) failed: {e}"))?;
    Ok(())
}

// ── the service bundle (explicit secret; no environment dependence) ─────────

/// Every hand engine over one pool, sharing one registry (base KPIs
/// registered), one recording mail seam, and one recipient-context
/// slot the probe installs an adapter into.
pub struct Svc {
    pub pool: PgPool,
    pub registry: Arc<KpiRegistry>,
    pub slot: RecipientContextSlot,
    pub write: Arc<DigestWriteService>,
    pub render: Arc<DigestRenderService>,
    pub cron: Arc<DigestCronService>,
    pub unsub: Arc<UnsubscribeService>,
    pub seam: Arc<RecordingMailSeam>,
}

impl Svc {
    pub fn new(pool: PgPool) -> Self {
        let slot = RecipientContextSlot::default();
        let registry = Arc::new(KpiRegistry::new());
        register_base_kpis(&registry, slot.clone());
        let seam = Arc::new(RecordingMailSeam::new());
        let unsub = Arc::new(UnsubscribeService::with_secret(pool.clone(), PROBE_SECRET));
        let write = Arc::new(DigestWriteService::new(pool.clone(), registry.clone()));
        let render = Arc::new(DigestRenderService::new(
            pool.clone(),
            registry.clone(),
            slot.clone(),
            seam.clone(),
            Some(unsub.clone()),
            "http://digest.probe.test",
        ));
        let cron = Arc::new(DigestCronService::new(pool.clone(), render.clone(), slot.clone()));
        Self { pool, registry, slot, write, render, cron, unsub, seam }
    }

    /// Wire the standard SQL adapter (sapiens identity facts over this
    /// pool) into the recipient slot.
    pub fn install_sql_port(&self) {
        self.slot.install(Arc::new(SqlRecipientContext::new(self.pool.clone())));
    }
}

// ── seeding helpers (direct SQL — tests may bypass the repositories) ────────

/// Insert a sapiens user row. `last_login` seeds the slowdown/connected
/// signals. Returns the user id.
pub async fn seed_user(pool: &PgPool, email: &str, last_login: Option<chrono::DateTime<chrono::Utc>>) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        r#"INSERT INTO users (id, username, email, password_hash, status, last_login)
           VALUES ($1, $2, $3, 'probe-hash', 'active', $4)"#,
    )
    .bind(id)
    .bind(email)
    .bind(email)
    .bind(last_login)
    .execute(pool)
    .await
    .expect("seed user");
    id
}

/// Insert an ACTIVE org-unit membership (the internal predicate; the
/// recipient port resolves the recipient's company from their active
/// memberships).
pub async fn seed_membership(pool: &PgPool, org_unit_id: Uuid, user_id: Uuid) {
    sqlx::query(
        r#"INSERT INTO sapiens.organization_users (org_unit_id, user_id, status)
           VALUES ($1, $2, 'active')"#,
    )
    .bind(org_unit_id)
    .bind(user_id)
    .execute(pool)
    .await
    .expect("seed membership");
}

/// Create a company-kind node under the tenant root and return its id —
/// the "company" a probe's memberships hang off. The sapiens membership
/// kind guard refuses any membership whose org-unit id does not reference
/// a real organization.org_units node of kind company or branch, so a
/// probe company must be a real node (its id doubles as the recipient
/// company id the port resolves).
pub async fn seed_org_unit(pool: &PgPool, code: &str, name: &str) -> Uuid {
    let id = Uuid::new_v4();
    let result = sqlx::query(
        r#"INSERT INTO organization.org_units (id, kind, parent_id, code, name)
           SELECT $1, 'company', r.id, $2, $3
           FROM organization.org_units r
           WHERE r.kind = 'root'"#,
    )
    .bind(id)
    .bind(code)
    .bind(name)
    .execute(pool)
    .await
    .expect("seed org unit");
    assert_eq!(
        result.rows_affected(),
        1,
        "seed org unit: tenant root node missing from the organization spine"
    );
    id
}

/// Insert one email-shaped mail message at `date` (the Messages Sent
/// volume). Returns its id.
pub async fn seed_mail_message(
    pool: &PgPool,
    date: chrono::DateTime<chrono::Utc>,
) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        r#"INSERT INTO messaging.mail_messages (id, subject, date, body, message_type)
           VALUES ($1, 'probe mail', $2, 'probe body', 'email')"#,
    )
    .bind(id)
    .bind(date)
    .execute(pool)
    .await
    .expect("seed mail message");
    id
}

/// Insert one carousel tip. `group_key` gates it (None = ungated).
pub async fn seed_tip(pool: &PgPool, sequence: i32, name: &str, description: &str, group_key: Option<&str>) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        r#"INSERT INTO digest.digest_tips (id, sequence, name, tip_description, group_key)
           VALUES ($1, $2, $3, $4, $5)"#,
    )
    .bind(id)
    .bind(sequence)
    .bind(name)
    .bind(description)
    .bind(group_key)
    .execute(pool)
    .await
    .expect("seed tip");
    id
}

/// The subscription row's state as the DATABASE spells it.
pub async fn subscription_state(pool: &PgPool, digest_id: Uuid, user_id: Uuid) -> String {
    sqlx::query_scalar::<_, String>(
        r#"SELECT state::text FROM digest.digest_subscriptions
           WHERE digest_id = $1 AND user_id = $2"#,
    )
    .bind(digest_id)
    .bind(user_id)
    .fetch_one(pool)
    .await
    .expect("subscription row")
}

/// One subscription row's metadata (the growth-loop markers ride it).
pub async fn subscription_metadata(
    pool: &PgPool,
    digest_id: Uuid,
    user_id: Uuid,
) -> serde_json::Value {
    sqlx::query_scalar::<_, serde_json::Value>(
        r#"SELECT metadata FROM digest.digest_subscriptions
           WHERE digest_id = $1 AND user_id = $2"#,
    )
    .bind(digest_id)
    .bind(user_id)
    .fetch_one(pool)
    .await
    .expect("subscription metadata")
}

/// The digest row's periodicity + next_run_date as the DATABASE spells
/// them (the ladder + advance witnesses).
pub async fn digest_cadence(
    pool: &PgPool,
    digest_id: Uuid,
) -> (String, Option<chrono::NaiveDate>) {
    sqlx::query_as::<_, (String, Option<chrono::NaiveDate>)>(
        r#"SELECT periodicity::text, next_run_date FROM digest.digest_digests WHERE id = $1"#,
    )
    .bind(digest_id)
    .fetch_one(pool)
    .await
    .expect("digest cadence row")
}

/// A synthetic `sapiens.user.created` envelope (the shape the host
/// relay delivers).
pub fn user_created_envelope(user_id: Uuid) -> backbone_messaging::IntegrationEventEnvelope {
    backbone_messaging::IntegrationEventEnvelope {
        id: Uuid::new_v4().to_string(),
        event_type: "sapiens.user.created".into(),
        source_context: "sapiens".into(),
        aggregate_id: user_id.to_string(),
        occurred_at: chrono::Utc::now(),
        published_at: chrono::Utc::now(),
        version: 1,
        correlation_id: None,
        causation_id: None,
        payload: serde_json::json!({ "user_id": user_id }),
    }
}
