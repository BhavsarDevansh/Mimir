//! Entity model and entity-type enum.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::Type;
use static_assertions::const_assert;

/// Classification of an entity node in the knowledge graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Type, Serialize, Deserialize)]
#[repr(i16)]
pub enum EntityType {
    Person = 1,
    Place = 2,
    Event = 3,
    Object = 4,
    Concept = 5,
    Organization = 6,
    Activity = 7,
}

const_assert!((EntityType::Person as i16) != 0);

/// A node in the knowledge graph representing a real-world thing or idea.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow, Serialize, Deserialize)]
pub struct Entity {
    pub id: i32,
    pub name: String,
    pub entity_type_id: i16,
    pub aliases: Option<String>, // JSON array
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
