use serde::{Deserialize, Serialize};
use sqlx::Type;
use std::str::FromStr;
#[cfg(feature = "openapi")]
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[cfg_attr(feature = "openapi", derive(ToSchema))]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "digest_state", rename_all = "snake_case")]
pub enum DigestState {
    Activated,
    Deactivated,
}

impl std::fmt::Display for DigestState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Activated => write!(f, "activated"),
            Self::Deactivated => write!(f, "deactivated"),
        }
    }
}

impl FromStr for DigestState {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "activated" => Ok(Self::Activated),
            "deactivated" => Ok(Self::Deactivated),
            _ => Err(format!("Unknown DigestState variant: {}", s)),
        }
    }
}

impl Default for DigestState {
    fn default() -> Self {
        Self::Activated
    }
}
