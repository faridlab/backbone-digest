use chrono::{DateTime, Utc, NaiveDate};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

use super::DigestPeriodicity;
use super::DigestState;
use super::AuditMetadata;

use crate::domain::state_machine::{digest_stateStateMachine, digest_stateState, StateMachineError};

/// Strongly-typed ID for DigestDigest
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DigestDigestId(pub Uuid);

impl DigestDigestId {
    pub fn new(id: Uuid) -> Self { Self(id) }
    pub fn generate() -> Self { Self(Uuid::new_v4()) }
    pub fn into_inner(self) -> Uuid { self.0 }
}

impl std::fmt::Display for DigestDigestId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for DigestDigestId {
    type Err = uuid::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(Uuid::parse_str(s)?))
    }
}

impl From<Uuid> for DigestDigestId {
    fn from(id: Uuid) -> Self { Self(id) }
}

impl From<DigestDigestId> for Uuid {
    fn from(id: DigestDigestId) -> Self { id.0 }
}

impl AsRef<Uuid> for DigestDigestId {
    fn as_ref(&self) -> &Uuid { &self.0 }
}

impl std::ops::Deref for DigestDigestId {
    type Target = Uuid;
    fn deref(&self) -> &Self::Target { &self.0 }
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct DigestDigest {
    pub id: Uuid,
    pub name: String,
    pub periodicity: DigestPeriodicity,
    pub next_run_date: Option<NaiveDate>,
    pub(crate) state: DigestState,
    pub company_id: Uuid,
    #[serde(default)]
    #[sqlx(json)]
    pub metadata: AuditMetadata,
}

impl DigestDigest {
    /// Create a builder for DigestDigest
    pub fn builder() -> DigestDigestBuilder {
        <DigestDigestBuilder as Default>::default()
    }

    /// Create a new DigestDigest with required fields
    pub fn new(name: String, periodicity: DigestPeriodicity, state: DigestState, company_id: Uuid) -> Self {
        Self {
            id: Uuid::new_v4(),
            name,
            periodicity,
            next_run_date: None,
            state,
            company_id,
            metadata: AuditMetadata::default(),
        }
    }

    /// Get the entity's unique identifier
    pub fn id(&self) -> &Uuid {
        &self.id
    }

    /// Get a strongly-typed ID for this entity
    pub fn typed_id(&self) -> DigestDigestId {
        DigestDigestId(self.id)
    }

    /// Get when this entity was created
    pub fn created_at(&self) -> Option<&DateTime<Utc>> {
        self.metadata.created_at.as_ref()
    }

    /// Get when this entity was last updated
    pub fn updated_at(&self) -> Option<&DateTime<Utc>> {
        self.metadata.updated_at.as_ref()
    }

    /// Check if this entity is soft deleted
    pub fn is_deleted(&self) -> bool {
        self.metadata.deleted_at.is_some()
    }

    /// Check if this entity is active (not deleted)
    pub fn is_active(&self) -> bool {
        self.metadata.deleted_at.is_none()
    }

    /// Get when this entity was deleted
    pub fn deleted_at(&self) -> Option<&DateTime<Utc>> {
        self.metadata.deleted_at.as_ref()
    }

    /// Get who created this entity
    pub fn created_by(&self) -> Option<&Uuid> {
        self.metadata.created_by.as_ref()
    }

    /// Get who last updated this entity
    pub fn updated_by(&self) -> Option<&Uuid> {
        self.metadata.updated_by.as_ref()
    }

    /// Get who deleted this entity
    pub fn deleted_by(&self) -> Option<&Uuid> {
        self.metadata.deleted_by.as_ref()
    }


    // ==========================================================
    // Fluent Setters (with_* for optional fields)
    // ==========================================================

    /// Set the next_run_date field (chainable)
    pub fn with_next_run_date(mut self, value: NaiveDate) -> Self {
        self.next_run_date = Some(value);
        self
    }

    // ==========================================================
    // State Machine
    // ==========================================================

    /// Transition to a new state via the state state machine.
    ///
    /// Returns `Err` if the transition is not permitted from the current state.
    /// Use this method instead of assigning `self.state` directly.
    pub fn transition_to(&mut self, new_state: digest_stateState) -> Result<(), StateMachineError> {
        let current = self.state.to_string().parse::<digest_stateState>()?;
        let mut sm = digest_stateStateMachine::from_state(current);
        sm.transition_to_state(new_state)?;
        self.state = new_state.to_string().parse::<DigestState>()
            .map_err(|e| StateMachineError::InvalidState(e.to_string()))?;
        Ok(())
    }

    // ==========================================================
    // Partial Update
    // ==========================================================

    /// Apply partial updates from a map of field name to JSON value
    pub fn apply_patch(&mut self, fields: std::collections::HashMap<String, serde_json::Value>) {
        for (key, value) in fields {
            match key.as_str() {
                "name" => {
                    if let Ok(v) = serde_json::from_value(value) { self.name = v; }
                }
                "periodicity" => {
                    if let Ok(v) = serde_json::from_value(value) { self.periodicity = v; }
                }
                "next_run_date" => {
                    if let Ok(v) = serde_json::from_value(value) { self.next_run_date = v; }
                }
                "company_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.company_id = v; }
                }
                _ => {} // ignore unknown fields
            }
        }
    }

    // <<< CUSTOM METHODS START >>>
    // <<< CUSTOM METHODS END >>>
}

impl super::Entity for DigestDigest {
    type Id = Uuid;

    fn entity_id(&self) -> &Self::Id {
        &self.id
    }

    fn entity_type() -> &'static str {
        "DigestDigest"
    }
}

impl backbone_core::PersistentEntity for DigestDigest {
    fn entity_id(&self) -> String {
        self.id.to_string()
    }
    fn set_entity_id(&mut self, id: String) {
        if let Ok(uuid) = uuid::Uuid::parse_str(&id) {
            self.id = uuid;
        }
    }
    fn created_at(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.metadata.created_at
    }
    fn set_created_at(&mut self, ts: chrono::DateTime<chrono::Utc>) {
        self.metadata.created_at = Some(ts);
    }
    fn updated_at(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.metadata.updated_at
    }
    fn set_updated_at(&mut self, ts: chrono::DateTime<chrono::Utc>) {
        self.metadata.updated_at = Some(ts);
    }
    fn deleted_at(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.metadata.deleted_at
    }
    fn set_deleted_at(&mut self, ts: Option<chrono::DateTime<chrono::Utc>>) {
        self.metadata.deleted_at = ts;
    }
}

impl backbone_orm::EntityRepoMeta for DigestDigest {
    fn column_types() -> std::collections::HashMap<String, String> {
        let mut m = std::collections::HashMap::new();
        m.insert("id".to_string(), "uuid".to_string());
        m.insert("company_id".to_string(), "uuid".to_string());
        m.insert("periodicity".to_string(), "digest_periodicity".to_string());
        m.insert("state".to_string(), "digest_state".to_string());
        m
    }
    fn search_fields() -> &'static [&'static str] {
        &["name"]
    }
    fn company_field() -> Option<&'static str> {
        Some("company_id")
    }
}

/// Builder for DigestDigest entity
///
/// Provides a fluent API for constructing DigestDigest instances.
/// System fields (id, metadata, timestamps) are auto-initialized.
#[derive(Debug, Clone, Default)]
pub struct DigestDigestBuilder {
    name: Option<String>,
    periodicity: Option<DigestPeriodicity>,
    next_run_date: Option<NaiveDate>,
    state: Option<DigestState>,
    company_id: Option<Uuid>,
}

impl DigestDigestBuilder {
    /// Set the name field (required)
    pub fn name(mut self, value: String) -> Self {
        self.name = Some(value);
        self
    }

    /// Set the periodicity field (default: `DigestPeriodicity::default()`)
    pub fn periodicity(mut self, value: DigestPeriodicity) -> Self {
        self.periodicity = Some(value);
        self
    }

    /// Set the next_run_date field (optional)
    pub fn next_run_date(mut self, value: NaiveDate) -> Self {
        self.next_run_date = Some(value);
        self
    }

    /// Set the state field (default: `DigestState::default()`)
    pub fn state(mut self, value: DigestState) -> Self {
        self.state = Some(value);
        self
    }

    /// Set the company_id field (required)
    pub fn company_id(mut self, value: Uuid) -> Self {
        self.company_id = Some(value);
        self
    }

    /// Build the DigestDigest entity
    ///
    /// Returns Err if any required field without a default is missing.
    pub fn build(self) -> Result<DigestDigest, String> {
        let name = self.name.ok_or_else(|| "name is required".to_string())?;
        let company_id = self.company_id.ok_or_else(|| "company_id is required".to_string())?;

        Ok(DigestDigest {
            id: Uuid::new_v4(),
            name,
            periodicity: self.periodicity.unwrap_or_default(),
            next_run_date: self.next_run_date,
            state: self.state.unwrap_or_default(),
            company_id,
            metadata: AuditMetadata::default(),
        })
    }
}
