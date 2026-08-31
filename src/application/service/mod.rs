//! Application services.
//!
//! The generated entity-service block (each a type alias over
//! `GenericCrudService`) plus the hand-written engine seats (user_owned
//! files; see metaphor.codegen.yaml). This file is the reconciled
//! bootstrap merge: the generated declarations sit outside the CUSTOM
//! markers, the hand seats inside.

pub mod error;
pub use error::{ServiceError, ServiceResult};

pub mod digest_digest_service;
pub mod digest_subscription_service;
pub mod digest_digest_kpi_service;
pub mod digest_tip_service;
pub mod digest_tip_user_service;

pub use digest_digest_service::DigestDigestService;
pub use digest_subscription_service::DigestSubscriptionService;
pub use digest_digest_kpi_service::DigestDigestKpiService;
pub use digest_tip_service::DigestTipService;
pub use digest_tip_user_service::DigestTipUserService;

// <<< CUSTOM
pub mod base_kpis;
pub mod digest_cron_service;
pub mod digest_error;
pub mod digest_mail_seam;
pub mod digest_render_service;
pub mod digest_write_service;
pub mod engagement_port;
pub mod kpi_registry;
pub mod unsubscribe_service;
pub mod user_created_handler;

pub use base_kpis::{register_base_kpis, TimeWindow};
pub use digest_cron_service::{DigestCronService, SweepOutcome};
pub use digest_error::DigestError;
pub use digest_mail_seam::{DefaultMailSeam, DigestMailSeam, OutgoingDigestMail, RecordingMailSeam};
pub use digest_render_service::{DigestRenderService, RenderPlan};
pub use digest_write_service::{DigestPeriodicity, DigestRow, DigestWriteService};
pub use engagement_port::{
    CannedRecipientContext, RecipientContext, RecipientContextPort, RecipientContextSlot,
    RecipientPortError, SqlRecipientContext,
};
pub use kpi_registry::{
    FnComputer, KpiComputer, KpiDefinition, KpiError, KpiFenceDeclaration, KpiQueryContext,
    KpiRegistry, KpiRegistrationError, KpiValue, KPI_CONNECTED_USERS, KPI_MESSAGES_SENT,
    KPI_NAME_PREFIX,
};
pub use unsubscribe_service::{TokenVerdict, UnsubscribeService, UnsubscribeToken};
pub use user_created_handler::UserCreatedHandler;
// END CUSTOM
