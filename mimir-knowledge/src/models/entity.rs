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
    DateTime = 8,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entity_type_discriminant_values() {
        assert_eq!(EntityType::Person as i16, 1);
        assert_eq!(EntityType::Place as i16, 2);
        assert_eq!(EntityType::Event as i16, 3);
        assert_eq!(EntityType::Object as i16, 4);
        assert_eq!(EntityType::Concept as i16, 5);
        assert_eq!(EntityType::Organization as i16, 6);
        assert_eq!(EntityType::Activity as i16, 7);
        assert_eq!(EntityType::DateTime as i16, 8);
    }

    #[test]
    fn entity_construction_basic() {
        let now = Utc::now();
        let e = Entity {
            id: 42,
            name: "Alice".into(),
            entity_type_id: EntityType::Person as i16,
            aliases: Some(r#"["Al","Ali"]"#.into()),
            created_at: now,
            updated_at: now,
        };
        assert_eq!(e.id, 42);
        assert_eq!(e.name, "Alice");
        assert_eq!(e.entity_type_id, 1);
    }
}
