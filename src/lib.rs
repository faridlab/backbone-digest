//! Digest Module — the periodic KPI-email engine (Odoo `digest` port).
//!
//! Generated skeleton (metaphor-schema) reconciled with the hand engine
//! seats: the generated part carries the entity services and route
//! composers; the hand surface (the KPI registry, the render engine,
//! the cron + ladder, the RFC 8058 unsubscribe, the growth loop) lives
//! inside `// <<< CUSTOM` markers (and the user_owned files listed in
//! metaphor.codegen.yaml).
//!
//! ```text
//! let digest = DigestModule::builder()
//!     .with_database(pool.clone())
//!     .with_token_secret(secret_bytes)          // or DIGEST_TOKEN_SECRET
//!     .with_public_base_url("https://mail.example.test")
//!     .with_default_digest(default_digest_id)   // arms the growth loop
//!     .build()?;
//! host_bus.register_handler(digest.user_created_handler()).await;
//! app = app.merge(digest.digest_public_routes());
//! ```

#![recursion_limit = "1024"]
#![allow(unused_imports)]

// Generated modules (application MUST be declared — the whole hand service
// surface lives under it; config/generated.rs and routes/generated.rs stay
// unreferenced generator artifacts, the family shape — see backbone-survey).
pub mod application;
pub mod domain;
pub mod exports;
pub mod handlers;
pub mod infrastructure;
pub mod presentation;
pub mod seeders;

// Re-exports for convenience - Domain entities
pub use domain::entity::*;

// Re-exports - Infrastructure
pub use infrastructure::persistence::*;

// Re-exports - Application services
pub use application::service::DigestDigestService;
pub use application::service::DigestSubscriptionService;
pub use application::service::DigestDigestKpiService;
pub use application::service::DigestTipService;
pub use application::service::DigestTipUserService;

use std::sync::Arc;
use axum::Router;
use sqlx::PgPool;
use uuid::Uuid;

// <<< CUSTOM FIELDS
/// The digest module: the generated entity-service set PLUS the hand
/// engine seats (KPI registry, render, cron, unsubscribe, growth loop)
/// over one pool. Cheap to hold behind an `Arc`.
pub struct DigestModule {
    // generated entity services
    pub(crate) digest_digest_service: Arc<DigestDigestService>,
    pub(crate) digest_subscription_service: Arc<DigestSubscriptionService>,
    pub(crate) digest_digest_kpi_service: Arc<DigestDigestKpiService>,
    pub(crate) digest_tip_service: Arc<DigestTipService>,
    pub(crate) digest_tip_user_service: Arc<DigestTipUserService>,

    // hand engine seats
    pub(crate) registry: Arc<application::service::kpi_registry::KpiRegistry>,
    /// The fail-closed recipient-context slot (deny-by-default until the
    /// host wires `set_recipient_port` / builder `with_recipient_port`).
    pub(crate) recipient_port: application::service::engagement_port::RecipientContextSlot,
    pub(crate) write_service: Arc<application::service::DigestWriteService>,
    pub(crate) render_service: Arc<application::service::DigestRenderService>,
    pub(crate) cron_service: Arc<application::service::DigestCronService>,
    /// None until a token secret is configured (builder arg or
    /// `DIGEST_TOKEN_SECRET`) — renders then refuse loudly rather than
    /// mailing without the unsubscribe leg.
    pub(crate) unsubscribe: Option<Arc<application::service::UnsubscribeService>>,
    /// The growth loop's default digest (config `digest.default_digest_id`
    /// + `digest.default_digest_emails` folded by the host into ONE key:
    /// Some(id) arms the loop, None switches it off).
    pub(crate) default_digest: Option<Uuid>,
}
// END CUSTOM

impl DigestModule {
    /// Create a new module builder.
    pub fn builder() -> DigestModuleBuilder {
        DigestModuleBuilder::new()
    }

    /// Mount ALL generated CRUD endpoints with NO domain validation —
    /// the fully **unguarded** surface (the family contract: compose a
    /// guarded router for any real deployment; use this only in
    /// trusted/admin/seeding contexts).
    pub fn all_crud_routes(&self) -> Router {
        use presentation::http::{
            create_digest_digest_routes,
            create_digest_subscription_routes,
            create_digest_digest_kpi_routes,
            create_digest_tip_routes,
            create_digest_tip_user_routes,
        };

        Router::new()
            .merge(create_digest_digest_routes(self.digest_digest_service.clone()))
            .merge(create_digest_subscription_routes(self.digest_subscription_service.clone()))
            .merge(create_digest_digest_kpi_routes(self.digest_digest_kpi_service.clone()))
            .merge(create_digest_tip_routes(self.digest_tip_service.clone()))
            .merge(create_digest_tip_user_routes(self.digest_tip_user_service.clone()))
    }

    /// Deprecated alias for [`Self::all_crud_routes`] (mounts unvalidated
    /// generic CRUD; prefer `readonly_routes()` + validated verbs).
    #[deprecated(note = "mounts unvalidated generic CRUD; prefer readonly_routes() + the write-service verbs, or all_crud_routes() for the full/unguarded surface")]
    pub fn routes(&self) -> Router {
        self.all_crud_routes()
    }

    /// Read-only routes for every entity (GET endpoints only) — the safe
    /// generated base; the hand write verbs and the public tree compose
    /// onto it.
    pub fn readonly_routes(&self) -> Router {
        use presentation::http::{
            create_digest_digest_read_routes,
            create_digest_subscription_read_routes,
            create_digest_digest_kpi_read_routes,
            create_digest_tip_read_routes,
            create_digest_tip_user_read_routes,
        };

        Router::new()
            .merge(create_digest_digest_read_routes(self.digest_digest_service.clone()))
            .merge(create_digest_subscription_read_routes(self.digest_subscription_service.clone()))
            .merge(create_digest_digest_kpi_read_routes(self.digest_digest_kpi_service.clone()))
            .merge(create_digest_tip_read_routes(self.digest_tip_service.clone()))
            .merge(create_digest_tip_user_read_routes(self.digest_tip_user_service.clone()))
    }

    // <<< CUSTOM METHODS
    /// The composed KPI registry — the API surface other modules extend
    /// (`register_kpi` also works post-build through this handle).
    pub fn kpi_registry(&self) -> &Arc<application::service::kpi_registry::KpiRegistry> {
        &self.registry
    }

    /// Register a KPI post-build (same registry the builder populated;
    /// interior-mutable so late-arriving modules can extend it). A
    /// duplicate name PANICS (R-DG1).
    pub fn register_kpi(
        &self,
        def: application::service::kpi_registry::KpiDefinition,
    ) -> Result<(), application::service::kpi_registry::KpiRegistrationError> {
        self.registry.register(def)
    }

    /// Wire the recipient-context port (the fail-closed identity seam).
    /// `SqlRecipientContext` is the standard adapter; until this is
    /// called the port refuses everything loudly.
    pub fn set_recipient_port(
        &self,
        port: Arc<dyn application::service::engagement_port::RecipientContextPort>,
    ) {
        self.recipient_port.install(port);
    }

    pub fn write_service(&self) -> &Arc<application::service::DigestWriteService> {
        &self.write_service
    }

    pub fn render_service(&self) -> &Arc<application::service::DigestRenderService> {
        &self.render_service
    }

    /// The daily-pull sweep + manual Send Now engine. The sweep is the
    /// `digest::send_due` cron handler body — the host schedules it
    /// (02:41 daily) on a cron pool that reads every due digest regardless
    /// of any request-scoped org fence (the cron-pool posture; see
    /// docs/port-notes.md).
    pub fn cron_service(&self) -> &Arc<application::service::DigestCronService> {
        &self.cron_service
    }

    /// The unsubscribe engine — `None` when no token secret is
    /// configured (the public routes then hold the RFC 8058 shape with
    /// no action; renders refuse loudly first, so no link exists).
    pub fn unsubscribe_service(&self) -> Option<&Arc<application::service::UnsubscribeService>> {
        self.unsubscribe.as_ref()
    }

    /// The growth-loop consumer (`sapiens.user.created`). Register on
    /// the HOST's integration bus:
    /// `bus.register_handler(module.user_created_handler()).await`.
    pub fn user_created_handler(&self) -> Arc<application::service::UserCreatedHandler> {
        Arc::new(application::service::UserCreatedHandler::new(
            self.write_service.clone(),
            self.recipient_port.clone(),
            self.default_digest,
        ))
    }

    /// The PUBLIC digest tree — the RFC 8058 one-click endpoints as BARE
    /// capability mounts (the token is the auth), throttled 120/min per
    /// client. Mount at the host ROOT (never behind a portal edge).
    pub fn digest_public_routes(self: &Arc<Self>) -> Router {
        crate::presentation::http::public_routes::public_composer()
            .with_state(Arc::clone(self))
    }
    // END CUSTOM
}

/// Builder for DigestModule.
pub struct DigestModuleBuilder {
    db_pool: Option<PgPool>,
    // <<< CUSTOM - custom builder fields
    token_secret: Option<Vec<u8>>,
    public_base_url: Option<String>,
    default_digest: Option<Uuid>,
    mail_seam: Option<Arc<dyn application::service::DigestMailSeam>>,
    recipient_port: Option<Arc<dyn application::service::engagement_port::RecipientContextPort>>,
    kpis: Vec<application::service::kpi_registry::KpiDefinition>,
    // END CUSTOM
}

impl DigestModuleBuilder {
    /// Create a new builder.
    pub fn new() -> Self {
        Self {
            db_pool: None,
            // <<< CUSTOM
            token_secret: None,
            public_base_url: None,
            default_digest: None,
            mail_seam: None,
            recipient_port: None,
            kpis: Vec::new(),
            // END CUSTOM
        }
    }

    /// Set the database connection pool.
    pub fn with_database(mut self, pool: PgPool) -> Self {
        self.db_pool = Some(pool);
        self
    }

    // <<< CUSTOM - custom builder methods
    /// The HMAC secret for unsubscribe tokens (Tier A capability
    /// minting). Falls back to `DIGEST_TOKEN_SECRET` at build; when
    /// neither is set the module builds WITHOUT the unsubscribe engine
    /// and every digest render refuses loudly (a digest mail without
    /// the unsubscribe leg must not send).
    pub fn with_token_secret(mut self, secret: impl Into<Vec<u8>>) -> Self {
        self.token_secret = Some(secret.into());
        self
    }

    /// The public base URL the mailed unsubscribe links carry
    /// (e.g. `https://mail.example.test`) — where the host mounts
    /// [`DigestModule::digest_public_routes`]. Default
    /// `http://localhost:8080` (dev posture; set it in real hosts).
    pub fn with_public_base_url(mut self, base: impl Into<String>) -> Self {
        self.public_base_url = Some(base.into());
        self
    }

    /// Arm the growth loop: subscribe every newly-created INTERNAL user
    /// to this digest (config keys `digest.default_digest_id` +
    /// `digest.default_digest_emails`, folded by the host into ONE
    /// setter — absence leaves the loop off, and every event is then an
    /// audited refusal, never a silent skip).
    pub fn with_default_digest(mut self, digest_id: Uuid) -> Self {
        self.default_digest = Some(digest_id);
        self
    }

    /// Override the outbound mail seam (default: backbone-mail's public
    /// message_post + queue enqueue). Probes inject the recording double
    /// here.
    pub fn with_mail_seam(
        mut self,
        seam: Arc<dyn application::service::DigestMailSeam>,
    ) -> Self {
        self.mail_seam = Some(seam);
        self
    }

    /// Wire the recipient-context port at build (the standard adapter
    /// is `SqlRecipientContext::new(pool)`); equivalent to calling
    /// `set_recipient_port` after build. Unwired = every identity fact
    /// refuses loudly (fail-closed).
    pub fn with_recipient_port(
        mut self,
        port: Arc<dyn application::service::engagement_port::RecipientContextPort>,
    ) -> Self {
        self.recipient_port = Some(port);
        self
    }

    /// Register a KPI into the module's registry (the extension surface
    /// other modules compose through). A duplicate name PANICS at build
    /// (R-DG1); a missing `kpi_` prefix refuses the build.
    pub fn register_kpi(
        mut self,
        def: application::service::kpi_registry::KpiDefinition,
    ) -> Self {
        self.kpis.push(def);
        self
    }
    // END CUSTOM

    /// Build the module with configured dependencies.
    pub fn build(self) -> anyhow::Result<DigestModule> {
        let db_pool = self
            .db_pool
            .ok_or_else(|| anyhow::anyhow!("Database pool not configured"))?;

        // Generated entity services (the survey shape: repository per
        // entity, service over the repository).
        let digest_digest_repository = Arc::new(DigestDigestRepository::new(db_pool.clone()));
        let digest_digest_service = Arc::new(DigestDigestService::with_repository(digest_digest_repository.clone()));

        let digest_subscription_repository = Arc::new(DigestSubscriptionRepository::new(db_pool.clone()));
        let digest_subscription_service = Arc::new(DigestSubscriptionService::with_repository(digest_subscription_repository.clone()));

        let digest_digest_kpi_repository = Arc::new(DigestDigestKpiRepository::new(db_pool.clone()));
        let digest_digest_kpi_service = Arc::new(DigestDigestKpiService::with_repository(digest_digest_kpi_repository.clone()));

        let digest_tip_repository = Arc::new(DigestTipRepository::new(db_pool.clone()));
        let digest_tip_service = Arc::new(DigestTipService::with_repository(digest_tip_repository.clone()));

        let digest_tip_user_repository = Arc::new(DigestTipUserRepository::new(db_pool.clone()));
        let digest_tip_user_service = Arc::new(DigestTipUserService::with_repository(digest_tip_user_repository.clone()));

        // <<< CUSTOM
        let registry = Arc::new(application::service::kpi_registry::KpiRegistry::new());
        let recipient_port = application::service::engagement_port::RecipientContextSlot::default();
        if let Some(port) = self.recipient_port {
            recipient_port.install(port);
        }

        // The base KPI pair first (their computers ride the port slot),
        // then the caller's registrations — a caller re-registering a
        // base name PANICS here at build time (R-DG1: a metric name is
        // an identity).
        application::service::base_kpis::register_base_kpis(&registry, recipient_port.clone());
        for def in self.kpis {
            registry
                .register(def)
                .map_err(|e| anyhow::anyhow!("KPI registration refused: {e}"))?;
        }

        let write_service = Arc::new(application::service::DigestWriteService::new(
            db_pool.clone(),
            registry.clone(),
        ));

        let mail_seam: Arc<dyn application::service::DigestMailSeam> =
            self.mail_seam.unwrap_or_else(|| {
                Arc::new(application::service::digest_mail_seam::DefaultMailSeam::new(
                    db_pool.clone(),
                ))
            });

        let secret = self.token_secret.clone().or_else(|| {
            std::env::var(application::service::unsubscribe_service::DIGEST_TOKEN_SECRET_ENV)
                .ok()
                .map(|s| s.into_bytes())
        });
        let unsubscribe = secret.map(|s| {
            Arc::new(application::service::UnsubscribeService::with_secret(
                db_pool.clone(),
                &s,
            ))
        });
        if unsubscribe.is_none() {
            tracing::warn!(
                "no unsubscribe token secret configured (builder or {}); digest renders will refuse until one is set",
                application::service::unsubscribe_service::DIGEST_TOKEN_SECRET_ENV
            );
        }

        let render_service = Arc::new(application::service::DigestRenderService::new(
            db_pool.clone(),
            registry.clone(),
            recipient_port.clone(),
            mail_seam,
            unsubscribe.clone(),
            self.public_base_url
                .clone()
                .unwrap_or_else(|| "http://localhost:8080".to_string()),
        ));
        let cron_service = Arc::new(application::service::DigestCronService::new(
            db_pool.clone(),
            render_service.clone(),
            recipient_port.clone(),
        ));
        // END CUSTOM

        Ok(DigestModule {
            digest_digest_service,
            digest_subscription_service,
            digest_digest_kpi_service,
            digest_tip_service,
            digest_tip_user_service,
            // <<< CUSTOM
            registry,
            recipient_port,
            write_service,
            render_service,
            cron_service,
            unsubscribe,
            default_digest: self.default_digest,
            // END CUSTOM
        })
    }
}

impl Default for DigestModuleBuilder {
    fn default() -> Self {
        Self::new()
    }
}
