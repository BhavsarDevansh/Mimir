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

/// Every [`EntityType`] variant in discriminant order.
///
/// Single source of truth for callers that must enumerate the variants (for
/// example, to derive a JSON Schema `enum` from the serde representation
/// instead of re-typing the variant names). Keep this in lock-step with the
/// enum: every variant appears exactly once.
pub const ENTITY_TYPES: [EntityType; 8] = [
    EntityType::Person,
    EntityType::Place,
    EntityType::Event,
    EntityType::Object,
    EntityType::Concept,
    EntityType::Organization,
    EntityType::Activity,
    EntityType::DateTime,
];

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
    fn entity_types_lists_every_variant_in_order() {
        // Single source of truth: the const array must enumerate the enum's
        // variants in discriminant order and stay in lock-step with it.
        assert_eq!(ENTITY_TYPES.len(), 8);
        assert_eq!(ENTITY_TYPES[0], EntityType::Person);
        assert_eq!(ENTITY_TYPES[1], EntityType::Place);
        assert_eq!(ENTITY_TYPES[2], EntityType::Event);
        assert_eq!(ENTITY_TYPES[3], EntityType::Object);
        assert_eq!(ENTITY_TYPES[4], EntityType::Concept);
        assert_eq!(ENTITY_TYPES[5], EntityType::Organization);
        assert_eq!(ENTITY_TYPES[6], EntityType::Activity);
        assert_eq!(ENTITY_TYPES[7], EntityType::DateTime);
        // No duplicates.
        let mut sorted = ENTITY_TYPES;
        sorted.sort_by_key(|t| *t as i16);
        assert_eq!(sorted, ENTITY_TYPES);
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
