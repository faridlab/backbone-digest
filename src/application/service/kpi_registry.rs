//! The declarative KPI registry — the module's extension API surface and
//! the authority on the out-of-fence render drop (hand-written;
//! user-owned; see `metaphor.codegen.yaml`).
//!
//! # API CONTRACT (binding — the out-of-fence drop)
//!
//! A KPI is a **named, registered compute function** keyed `kpi_<name>`.
//! Every registry entry carries its OWN fence declaration — a statement of
//! WHICH company's (or shared) data the compute reads. At render time the
//! engine decides whether a KPI renders for a recipient FROM THAT
//! DECLARATION, **never from row count**. Under row-level security an
//! out-of-fence read is ZERO ROWS, indistinguishable from a genuine zero —
//! so any drop inferred from empty results would silently erase genuine
//! zeros. The two cases are observably different here:
//!
//! - an in-fence KPI whose data is genuinely zero **renders `0`**;
//! - an out-of-fence KPI is **absent from the rendered mail** — silently,
//!   by design (no placeholder, no note), mirroring Odoo's
//!   AccessError-drops-KPI behavior under a real fence.
//!
//! The drop decision is made BEFORE the compute runs: an out-of-fence
//! entry's compute function is never invoked, so no row count can be part
//! of the decision even accidentally.
//!
//! # Registering a KPI (the surface other modules extend)
//!
//! ```text
//! DigestModule::builder()
//!     .with_database(pool)
//!     .register_kpi(KpiDefinition {
//!         name: "kpi_crm_leads".into(),          // MUST start with "kpi_"
//!         label: "New Leads".into(),
//!         fence: KpiFenceDeclaration::CompanyData,
//!         computer: Arc::new(MyLeadsKpi),
//!     })
//!     .build()?;
//! ```
//!
//! `KpiFenceDeclaration::CompanyData` declares that the compute reads the
//! digest's own company's data — it renders only for recipients whose own
//! company fence covers that company. `SharedData` declares unfenced data
//! (renders for every recipient). `PinnedCompanyData(c)` declares data
//! pinned to one named company and renders only for that company's
//! recipients. Registration is also possible after `build()` through
//! [`DigestModule::register_kpi`] (the same registry, interior-mutable so
//! late-arriving modules can extend it).
//!
//! Re-registering an existing name PANICS at load time (R-DG1): the name
//! is the metric's identity, and two live compute functions claiming one
//! name is a composition bug, not a preference — the registry is keyed
//! by name, the Odoo field-namespace convention without the runtime
//! `_fields` scan (DG-1's port decision).

use std::collections::BTreeMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

/// A registered KPI's name MUST carry this prefix (the Odoo `kpi_*`
/// field-pair convention, ported as a naming rule instead of a scan).
pub const KPI_NAME_PREFIX: &str = "kpi_";

/// The base pair 1 of 2: Connected Users (sapiens `users.last_login`).
pub const KPI_CONNECTED_USERS: &str = "kpi_res_users_connected";
/// The base pair 2 of 2: Messages Sent (`messaging.mail_messages`).
pub const KPI_MESSAGES_SENT: &str = "kpi_mail_message_total";

/// What one compute invocation sees. The `company_id` is the fence the
/// value is computed under — the RECIPIENT's own company (never the
/// sending system's): KPI SQL must scope every company-bearing read with
/// it explicitly.
#[derive(Debug, Clone)]
pub struct KpiQueryContext {
    pub recipient_user_id: Uuid,
    pub company_id: Uuid,
    pub window_start: DateTime<Utc>,
    pub window_end: DateTime<Utc>,
}

/// The computed value of one KPI over one window. Counts are the base
/// shape; money/float KPIs can use `amount` and render implementations
/// format it (the registry stores numbers, never strings).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct KpiValue {
    pub count: i64,
    pub amount: Option<f64>,
}

impl KpiValue {
    pub fn zero() -> Self {
        Self::default()
    }

    pub fn count(n: i64) -> Self {
        Self { count: n, amount: None }
    }

    /// The number the render table shows (amount when present, else count).
    pub fn display(&self) -> f64 {
        self.amount.unwrap_or(self.count as f64)
    }
}

/// A KPI compute failure. `Unavailable` is the benign shape (the source
/// table missing in this composition, say) — the engine DROPS the KPI from
/// that render with a warning, exactly like an out-of-fence entry; it never
/// fails the whole mail.
#[derive(Debug, thiserror::Error)]
pub enum KpiError {
    #[error("kpi compute failed: {0}")]
    Compute(#[source] sqlx::Error),
    #[error("kpi source unavailable: {0}")]
    Unavailable(String),
}

/// One registered compute function. Implementations run under the
/// recipient's fence (see [`KpiQueryContext`]) and MUST scope every
/// company-bearing read by `ctx.company_id`.
#[async_trait::async_trait]
pub trait KpiComputer: Send + Sync {
    async fn compute(&self, pool: &PgPool, ctx: &KpiQueryContext) -> Result<KpiValue, KpiError>;
}

/// A closure-backed computer for lightweight registrations.
pub struct FnComputer<F>(pub F);

#[async_trait::async_trait]
impl<F, Fut> KpiComputer for FnComputer<F>
where
    F: Fn(&PgPool, KpiQueryContext) -> Fut + Send + Sync,
    Fut: std::future::Future<Output = Result<KpiValue, KpiError>> + Send,
{
    async fn compute(&self, pool: &PgPool, ctx: &KpiQueryContext) -> Result<KpiValue, KpiError> {
        (self.0)(pool, ctx.clone()).await
    }
}

/// The fence declaration every registry entry carries. This is the ONLY
/// input to the render drop decision (see the API CONTRACT above).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KpiFenceDeclaration {
    /// The compute reads data that is not company-fenced — renders for
    /// every recipient.
    SharedData,
    /// The compute reads the DIGEST's own company's data — renders only
    /// for recipients whose own company fence covers the digest's company.
    CompanyData,
    /// The compute reads one named company's data (a KPI registered
    /// against a fixed company) — renders only for that company's
    /// recipients.
    PinnedCompanyData(Uuid),
}

impl KpiFenceDeclaration {
    /// The declaration-driven drop decision. PURE — probe-asserted
    /// directly. `recipient_company` is the recipient's own fence; this
    /// NEVER inspects a computed value or row count.
    pub fn renders_for(
        &self,
        recipient_company: Option<Uuid>,
        digest_company: Option<Uuid>,
    ) -> bool {
        match self {
            KpiFenceDeclaration::SharedData => true,
            KpiFenceDeclaration::CompanyData => match (recipient_company, digest_company) {
                (Some(r), Some(d)) => r == d,
                // No company on either side means the fence cannot be
                // established — fail CLOSED (drop), never render.
                _ => false,
            },
            KpiFenceDeclaration::PinnedCompanyData(pinned) => {
                recipient_company == Some(*pinned)
            }
        }
    }
}

/// One registered KPI: name + label + fence declaration + compute.
#[derive(Clone)]
pub struct KpiDefinition {
    /// MUST start with `kpi_` (enforced at registration).
    pub name: String,
    /// Human label for the rendered table.
    pub label: String,
    /// Which data the compute reads — the render-drop authority.
    pub fence: KpiFenceDeclaration,
    /// The compute function.
    pub computer: Arc<dyn KpiComputer>,
}

impl std::fmt::Debug for KpiDefinition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KpiDefinition")
            .field("name", &self.name)
            .field("label", &self.label)
            .field("fence", &self.fence)
            .finish_non_exhaustive()
    }
}

/// The registry: name → definition, interior-mutable so the builder and
/// post-build registrations share one map. Ordered by name for stable
/// iteration.
#[derive(Debug, Default)]
pub struct KpiRegistry {
    entries: std::sync::RwLock<BTreeMap<String, KpiDefinition>>,
}

/// A registration refusal (typed, never a panic).
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum KpiRegistrationError {
    #[error("KPI name '{0}' must start with '{1}'")]
    BadPrefix(String, String),
}

impl KpiRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register one KPI. The name must carry the `kpi_` prefix — the
    /// ported naming convention, enforced loudly. A DUPLICATE name is a
    /// load-time PANIC (R-DG1): two compute functions claiming one metric
    /// identity is a composition bug, never a preference.
    pub fn register(
        &self,
        def: KpiDefinition,
    ) -> Result<(), KpiRegistrationError> {
        if !def.name.starts_with(KPI_NAME_PREFIX) {
            return Err(KpiRegistrationError::BadPrefix(
                def.name,
                KPI_NAME_PREFIX.to_string(),
            ));
        }
        let mut guard = self.entries.write().unwrap_or_else(|e| e.into_inner());
        if guard.contains_key(&def.name) {
            panic!(
                "duplicate KPI registration: '{}' is already registered — \
                 a metric name is an identity, not a slot (R-DG1)",
                def.name
            );
        }
        guard.insert(def.name.clone(), def);
        Ok(())
    }

    /// Look one KPI up by name.
    pub fn get(&self, name: &str) -> Option<KpiDefinition> {
        self.entries
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(name)
            .cloned()
    }

    /// All registered names, ordered.
    pub fn names(&self) -> Vec<String> {
        self.entries
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .keys()
            .cloned()
            .collect()
    }

    /// Number of registered KPIs.
    pub fn len(&self) -> usize {
        self.entries
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct StubComputer;

    #[async_trait::async_trait]
    impl KpiComputer for StubComputer {
        async fn compute(&self, _pool: &PgPool, _ctx: &KpiQueryContext) -> Result<KpiValue, KpiError> {
            Ok(KpiValue::count(7))
        }
    }

    fn def(name: &str, fence: KpiFenceDeclaration) -> KpiDefinition {
        KpiDefinition {
            name: name.into(),
            label: name.into(),
            fence,
            computer: Arc::new(StubComputer),
        }
    }

    #[test]
    fn prefix_is_enforced() {
        let r = KpiRegistry::new();
        assert_eq!(
            r.register(def("crm_leads", KpiFenceDeclaration::SharedData)),
            Err(KpiRegistrationError::BadPrefix(
                "crm_leads".into(),
                "kpi_".into()
            ))
        );
        assert!(r.register(def("kpi_crm_leads", KpiFenceDeclaration::SharedData)).is_ok());
    }

    #[test]
    #[should_panic(expected = "duplicate KPI registration")]
    fn duplicate_registration_panics() {
        let r = KpiRegistry::new();
        r.register(def("kpi_crm_leads", KpiFenceDeclaration::SharedData)).unwrap();
        // A second compute function claiming the same metric identity is a
        // composition bug — a load-time panic, never a silent replace.
        let _ = r.register(def("kpi_crm_leads", KpiFenceDeclaration::CompanyData));
    }

    #[test]
    fn shared_renders_for_everyone() {
        assert!(KpiFenceDeclaration::SharedData.renders_for(None, None));
        assert!(KpiFenceDeclaration::SharedData.renders_for(None, Some(Uuid::new_v4())));
    }

    #[test]
    fn company_data_renders_only_when_fences_match() {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let f = KpiFenceDeclaration::CompanyData;
        assert!(f.renders_for(Some(a), Some(a)));
        assert!(!f.renders_for(Some(a), Some(b)));
        // Fail closed when either fence is unknown.
        assert!(!f.renders_for(None, Some(a)));
        assert!(!f.renders_for(Some(a), None));
    }

    #[test]
    fn pinned_company_renders_only_for_that_company() {
        let pinned = Uuid::new_v4();
        let f = KpiFenceDeclaration::PinnedCompanyData(pinned);
        assert!(f.renders_for(Some(pinned), Some(Uuid::new_v4())));
        assert!(!f.renders_for(Some(Uuid::new_v4()), Some(pinned)));
        assert!(!f.renders_for(None, None));
    }
}
