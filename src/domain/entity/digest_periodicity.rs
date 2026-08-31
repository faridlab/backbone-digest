use serde::{Deserialize, Serialize};
use sqlx::Type;
use std::str::FromStr;
#[cfg(feature = "openapi")]
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[cfg_attr(feature = "openapi", derive(ToSchema))]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "digest_periodicity", rename_all = "snake_case")]
pub enum DigestPeriodicity {
    Daily,
    Weekly,
    Monthly,
    Quarterly,
}

impl std::fmt::Display for DigestPeriodicity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Daily => write!(f, "daily"),
            Self::Weekly => write!(f, "weekly"),
            Self::Monthly => write!(f, "monthly"),
            Self::Quarterly => write!(f, "quarterly"),
        }
    }
}

impl FromStr for DigestPeriodicity {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "daily" => Ok(Self::Daily),
            "weekly" => Ok(Self::Weekly),
            "monthly" => Ok(Self::Monthly),
            "quarterly" => Ok(Self::Quarterly),
            _ => Err(format!("Unknown DigestPeriodicity variant: {}", s)),
        }
    }
}

impl Default for DigestPeriodicity {
    fn default() -> Self {
        Self::Daily
    }
}
