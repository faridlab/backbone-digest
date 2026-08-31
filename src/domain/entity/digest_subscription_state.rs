use serde::{Deserialize, Serialize};
use sqlx::Type;
use std::str::FromStr;
#[cfg(feature = "openapi")]
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[cfg_attr(feature = "openapi", derive(ToSchema))]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "digest_subscription_state", rename_all = "snake_case")]
pub enum DigestSubscriptionState {
    Subscribed,
    Unsubscribed,
}

impl std::fmt::Display for DigestSubscriptionState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Subscribed => write!(f, "subscribed"),
            Self::Unsubscribed => write!(f, "unsubscribed"),
        }
    }
}

impl FromStr for DigestSubscriptionState {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "subscribed" => Ok(Self::Subscribed),
            "unsubscribed" => Ok(Self::Unsubscribed),
            _ => Err(format!("Unknown DigestSubscriptionState variant: {}", s)),
        }
    }
}

impl Default for DigestSubscriptionState {
    fn default() -> Self {
        Self::Subscribed
    }
}
