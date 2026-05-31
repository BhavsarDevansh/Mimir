//! Entity location model.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A location associated with an entity.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow, Serialize, Deserialize)]
pub struct EntityLocation {
    pub id: i32,
    pub entity_id: i32,
    pub location_type_id: i16,
    pub address: Option<String>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub timezone: Option<String>,
    pub valid_from: Option<DateTime<Utc>>,
    pub valid_until: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}
