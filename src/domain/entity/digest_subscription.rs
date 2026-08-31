use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

use super::DigestSubscriptionState;
use super::AuditMetadata;

use crate::domain::state_machine::{digest_subscription_stateStateMachine, digest_subscription_stateState, StateMachineError};

/// Strongly-typed ID for DigestSubscription
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DigestSubscriptionId(pub Uuid);

impl DigestSubscriptionId {
    pub fn new(id: Uuid) -> Self { Self(id) }
    pub fn generate() -> Self { Self(Uuid::new_v4()) }
    pub fn into_inner(self) -> Uuid { self.0 }
}

impl std::fmt::Display for DigestSubscriptionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for DigestSubscriptionId {
    type Err = uuid::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(Uuid::parse_str(s)?))
    }
}

impl From<Uuid> for DigestSubscriptionId {
    fn from(id: Uuid) -> Self { Self(id) }
}

impl From<DigestSubscriptionId> for Uuid {
    fn from(id: DigestSubscriptionId) -> Self { id.0 }
}

impl AsRef<Uuid> for DigestSubscriptionId {
    fn as_ref(&self) -> &Uuid { &self.0 }
}

impl std::ops::Deref for DigestSubscriptionId {
    type Target = Uuid;
    fn deref(&self) -> &Self::Target { &self.0 }
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct DigestSubscription {
    pub id: Uuid,
    pub digest_id: Uuid,
    pub user_id: Uuid,
    pub(crate) state: DigestSubscriptionState,
    pub unsubscribed_at: Option<DateTime<Utc>>,
    #[serde(default)]
    #[sqlx(json)]
    pub metadata: AuditMetadata,
}

impl DigestSubscription {
    /// Create a builder for DigestSubscription
    pub fn builder() -> DigestSubscriptionBuilder {
        <DigestSubscriptionBuilder as Default>::default()
    }

    /// Create a new DigestSubscription with required fields
    pub fn new(digest_id: Uuid, user_id: Uuid, state: DigestSubscriptionState) -> Self {
        Self {
            id: Uuid::new_v4(),
            digest_id,
            user_id,
            state,
            unsubscribed_at: None,
            metadata: AuditMetadata::default(),
        }
    }

    /// Get the entity's unique identifier
    pub fn id(&self) -> &Uuid {
        &self.id
    }

    /// Get a strongly-typed ID for this entity
    pub fn typed_id(&self) -> DigestSubscriptionId {
        DigestSubscriptionId(self.id)
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

    /// Set the unsubscribed_at field (chainable)
    pub fn with_unsubscribed_at(mut self, value: DateTime<Utc>) -> Self {
        self.unsubscribed_at = Some(value);
        self
    }

    // ==========================================================
    // State Machine
    // ==========================================================

    /// Transition to a new state via the state state machine.
    ///
    /// Returns `Err` if the transition is not permitted from the current state.
    /// Use this method instead of assigning `self.state` directly.
    pub fn transition_to(&mut self, new_state: digest_subscription_stateState) -> Result<(), StateMachineError> {
        let current = self.state.to_string().parse::<digest_subscription_stateState>()?;
        let mut sm = digest_subscription_stateStateMachine::from_state(current);
        sm.transition_to_state(new_state)?;
        self.state = new_state.to_string().parse::<DigestSubscriptionState>()
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
                "digest_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.digest_id = v; }
                }
                "user_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.user_id = v; }
                }
                "unsubscribed_at" => {
                    if let Ok(v) = serde_json::from_value(value) { self.unsubscribed_at = v; }
                }
                _ => {} // ignore unknown fields
            }
        }
    }

    // <<< CUSTOM METHODS START >>>
    // <<< CUSTOM METHODS END >>>
}

impl super::Entity for DigestSubscription {
    type Id = Uuid;

    fn entity_id(&self) -> &Self::Id {
        &self.id
    }

    fn entity_type() -> &'static str {
        "DigestSubscription"
    }
}

impl backbone_core::PersistentEntity for DigestSubscription {
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

impl backbone_orm::EntityRepoMeta for DigestSubscription {
    fn column_types() -> std::collections::HashMap<String, String> {
        let mut m = std::collections::HashMap::new();
        m.insert("id".to_string(), "uuid".to_string());
        m.insert("digest_id".to_string(), "uuid".to_string());
        m.insert("user_id".to_string(), "uuid".to_string());
        m.insert("state".to_string(), "digest_subscription_state".to_string());
        m
    }
    fn search_fields() -> &'static [&'static str] {
        &[]
    }
    fn relations() -> &'static [(&'static str, &'static str, &'static str)] {
        &[("digest", "digest_digests", "digestId")]
    }
}

/// Builder for DigestSubscription entity
///
/// Provides a fluent API for constructing DigestSubscription instances.
/// System fields (id, metadata, timestamps) are auto-initialized.
#[derive(Debug, Clone, Default)]
pub struct DigestSubscriptionBuilder {
    digest_id: Option<Uuid>,
    user_id: Option<Uuid>,
    state: Option<DigestSubscriptionState>,
    unsubscribed_at: Option<DateTime<Utc>>,
}

impl DigestSubscriptionBuilder {
    /// Set the digest_id field (required)
    pub fn digest_id(mut self, value: Uuid) -> Self {
        self.digest_id = Some(value);
        self
    }

    /// Set the user_id field (required)
    pub fn user_id(mut self, value: Uuid) -> Self {
        self.user_id = Some(value);
        self
    }

    /// Set the state field (default: `DigestSubscriptionState::default()`)
    pub fn state(mut self, value: DigestSubscriptionState) -> Self {
        self.state = Some(value);
        self
    }

    /// Set the unsubscribed_at field (optional)
    pub fn unsubscribed_at(mut self, value: DateTime<Utc>) -> Self {
        self.unsubscribed_at = Some(value);
        self
    }

    /// Build the DigestSubscription entity
    ///
    /// Returns Err if any required field without a default is missing.
    pub fn build(self) -> Result<DigestSubscription, String> {
        let digest_id = self.digest_id.ok_or_else(|| "digest_id is required".to_string())?;
        let user_id = self.user_id.ok_or_else(|| "user_id is required".to_string())?;

        Ok(DigestSubscription {
            id: Uuid::new_v4(),
            digest_id,
            user_id,
            state: self.state.unwrap_or_default(),
            unsubscribed_at: self.unsubscribed_at,
            metadata: AuditMetadata::default(),
        })
    }
}
