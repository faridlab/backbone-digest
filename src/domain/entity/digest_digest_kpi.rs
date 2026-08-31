use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;
use super::AuditMetadata;

/// Strongly-typed ID for DigestDigestKpi
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DigestDigestKpiId(pub Uuid);

impl DigestDigestKpiId {
    pub fn new(id: Uuid) -> Self { Self(id) }
    pub fn generate() -> Self { Self(Uuid::new_v4()) }
    pub fn into_inner(self) -> Uuid { self.0 }
}

impl std::fmt::Display for DigestDigestKpiId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for DigestDigestKpiId {
    type Err = uuid::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(Uuid::parse_str(s)?))
    }
}

impl From<Uuid> for DigestDigestKpiId {
    fn from(id: Uuid) -> Self { Self(id) }
}

impl From<DigestDigestKpiId> for Uuid {
    fn from(id: DigestDigestKpiId) -> Self { id.0 }
}

impl AsRef<Uuid> for DigestDigestKpiId {
    fn as_ref(&self) -> &Uuid { &self.0 }
}

impl std::ops::Deref for DigestDigestKpiId {
    type Target = Uuid;
    fn deref(&self) -> &Self::Target { &self.0 }
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct DigestDigestKpi {
    pub id: Uuid,
    pub digest_id: Uuid,
    pub kpi_key: String,
    #[serde(default)]
    #[sqlx(json)]
    pub metadata: AuditMetadata,
}

impl DigestDigestKpi {
    /// Create a builder for DigestDigestKpi
    pub fn builder() -> DigestDigestKpiBuilder {
        <DigestDigestKpiBuilder as Default>::default()
    }

    /// Create a new DigestDigestKpi with required fields
    pub fn new(digest_id: Uuid, kpi_key: String) -> Self {
        Self {
            id: Uuid::new_v4(),
            digest_id,
            kpi_key,
            metadata: AuditMetadata::default(),
        }
    }

    /// Get the entity's unique identifier
    pub fn id(&self) -> &Uuid {
        &self.id
    }

    /// Get a strongly-typed ID for this entity
    pub fn typed_id(&self) -> DigestDigestKpiId {
        DigestDigestKpiId(self.id)
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
    // Partial Update
    // ==========================================================

    /// Apply partial updates from a map of field name to JSON value
    pub fn apply_patch(&mut self, fields: std::collections::HashMap<String, serde_json::Value>) {
        for (key, value) in fields {
            match key.as_str() {
                "digest_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.digest_id = v; }
                }
                "kpi_key" => {
                    if let Ok(v) = serde_json::from_value(value) { self.kpi_key = v; }
                }
                _ => {} // ignore unknown fields
            }
        }
    }

    // <<< CUSTOM METHODS START >>>
    // <<< CUSTOM METHODS END >>>
}

impl super::Entity for DigestDigestKpi {
    type Id = Uuid;

    fn entity_id(&self) -> &Self::Id {
        &self.id
    }

    fn entity_type() -> &'static str {
        "DigestDigestKpi"
    }
}

impl backbone_core::PersistentEntity for DigestDigestKpi {
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

impl backbone_orm::EntityRepoMeta for DigestDigestKpi {
    fn column_types() -> std::collections::HashMap<String, String> {
        let mut m = std::collections::HashMap::new();
        m.insert("id".to_string(), "uuid".to_string());
        m.insert("digest_id".to_string(), "uuid".to_string());
        m
    }
    fn search_fields() -> &'static [&'static str] {
        &["kpi_key"]
    }
    fn relations() -> &'static [(&'static str, &'static str, &'static str)] {
        &[("digest", "digest_digests", "digestId")]
    }
}

/// Builder for DigestDigestKpi entity
///
/// Provides a fluent API for constructing DigestDigestKpi instances.
/// System fields (id, metadata, timestamps) are auto-initialized.
#[derive(Debug, Clone, Default)]
pub struct DigestDigestKpiBuilder {
    digest_id: Option<Uuid>,
    kpi_key: Option<String>,
}

impl DigestDigestKpiBuilder {
    /// Set the digest_id field (required)
    pub fn digest_id(mut self, value: Uuid) -> Self {
        self.digest_id = Some(value);
        self
    }

    /// Set the kpi_key field (required)
    pub fn kpi_key(mut self, value: String) -> Self {
        self.kpi_key = Some(value);
        self
    }

    /// Build the DigestDigestKpi entity
    ///
    /// Returns Err if any required field without a default is missing.
    pub fn build(self) -> Result<DigestDigestKpi, String> {
        let digest_id = self.digest_id.ok_or_else(|| "digest_id is required".to_string())?;
        let kpi_key = self.kpi_key.ok_or_else(|| "kpi_key is required".to_string())?;

        Ok(DigestDigestKpi {
            id: Uuid::new_v4(),
            digest_id,
            kpi_key,
            metadata: AuditMetadata::default(),
        })
    }
}
