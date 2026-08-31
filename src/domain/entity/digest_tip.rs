use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;
use super::AuditMetadata;

/// Strongly-typed ID for DigestTip
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DigestTipId(pub Uuid);

impl DigestTipId {
    pub fn new(id: Uuid) -> Self { Self(id) }
    pub fn generate() -> Self { Self(Uuid::new_v4()) }
    pub fn into_inner(self) -> Uuid { self.0 }
}

impl std::fmt::Display for DigestTipId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for DigestTipId {
    type Err = uuid::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(Uuid::parse_str(s)?))
    }
}

impl From<Uuid> for DigestTipId {
    fn from(id: Uuid) -> Self { Self(id) }
}

impl From<DigestTipId> for Uuid {
    fn from(id: DigestTipId) -> Self { id.0 }
}

impl AsRef<Uuid> for DigestTipId {
    fn as_ref(&self) -> &Uuid { &self.0 }
}

impl std::ops::Deref for DigestTipId {
    type Target = Uuid;
    fn deref(&self) -> &Self::Target { &self.0 }
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct DigestTip {
    pub id: Uuid,
    pub sequence: i32,
    pub name: String,
    pub tip_description: String,
    pub group_key: Option<String>,
    #[serde(default)]
    #[sqlx(json)]
    pub metadata: AuditMetadata,
}

impl DigestTip {
    /// Create a builder for DigestTip
    pub fn builder() -> DigestTipBuilder {
        <DigestTipBuilder as Default>::default()
    }

    /// Create a new DigestTip with required fields
    pub fn new(sequence: i32, name: String, tip_description: String) -> Self {
        Self {
            id: Uuid::new_v4(),
            sequence,
            name,
            tip_description,
            group_key: None,
            metadata: AuditMetadata::default(),
        }
    }

    /// Get the entity's unique identifier
    pub fn id(&self) -> &Uuid {
        &self.id
    }

    /// Get a strongly-typed ID for this entity
    pub fn typed_id(&self) -> DigestTipId {
        DigestTipId(self.id)
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

    /// Set the group_key field (chainable)
    pub fn with_group_key(mut self, value: String) -> Self {
        self.group_key = Some(value);
        self
    }

    // ==========================================================
    // Partial Update
    // ==========================================================

    /// Apply partial updates from a map of field name to JSON value
    pub fn apply_patch(&mut self, fields: std::collections::HashMap<String, serde_json::Value>) {
        for (key, value) in fields {
            match key.as_str() {
                "sequence" => {
                    if let Ok(v) = serde_json::from_value(value) { self.sequence = v; }
                }
                "name" => {
                    if let Ok(v) = serde_json::from_value(value) { self.name = v; }
                }
                "tip_description" => {
                    if let Ok(v) = serde_json::from_value(value) { self.tip_description = v; }
                }
                "group_key" => {
                    if let Ok(v) = serde_json::from_value(value) { self.group_key = v; }
                }
                _ => {} // ignore unknown fields
            }
        }
    }

    // <<< CUSTOM METHODS START >>>
    // <<< CUSTOM METHODS END >>>
}

impl super::Entity for DigestTip {
    type Id = Uuid;

    fn entity_id(&self) -> &Self::Id {
        &self.id
    }

    fn entity_type() -> &'static str {
        "DigestTip"
    }
}

impl backbone_core::PersistentEntity for DigestTip {
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

impl backbone_orm::EntityRepoMeta for DigestTip {
    fn column_types() -> std::collections::HashMap<String, String> {
        let mut m = std::collections::HashMap::new();
        m.insert("id".to_string(), "uuid".to_string());
        m
    }
    fn search_fields() -> &'static [&'static str] {
        &["name", "tip_description"]
    }
}

/// Builder for DigestTip entity
///
/// Provides a fluent API for constructing DigestTip instances.
/// System fields (id, metadata, timestamps) are auto-initialized.
#[derive(Debug, Clone, Default)]
pub struct DigestTipBuilder {
    sequence: Option<i32>,
    name: Option<String>,
    tip_description: Option<String>,
    group_key: Option<String>,
}

impl DigestTipBuilder {
    /// Set the sequence field (default: `1`)
    pub fn sequence(mut self, value: i32) -> Self {
        self.sequence = Some(value);
        self
    }

    /// Set the name field (required)
    pub fn name(mut self, value: String) -> Self {
        self.name = Some(value);
        self
    }

    /// Set the tip_description field (required)
    pub fn tip_description(mut self, value: String) -> Self {
        self.tip_description = Some(value);
        self
    }

    /// Set the group_key field (optional)
    pub fn group_key(mut self, value: String) -> Self {
        self.group_key = Some(value);
        self
    }

    /// Build the DigestTip entity
    ///
    /// Returns Err if any required field without a default is missing.
    pub fn build(self) -> Result<DigestTip, String> {
        let name = self.name.ok_or_else(|| "name is required".to_string())?;
        let tip_description = self.tip_description.ok_or_else(|| "tip_description is required".to_string())?;

        Ok(DigestTip {
            id: Uuid::new_v4(),
            sequence: self.sequence.unwrap_or(1),
            name,
            tip_description,
            group_key: self.group_key,
            metadata: AuditMetadata::default(),
        })
    }
}
