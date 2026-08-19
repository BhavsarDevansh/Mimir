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

impl TryFrom<i16> for EntityType {
    type Error = ();

    fn try_from(value: i16) -> Result<Self, Self::Error> {
        match value {
            x if x == Self::Person as i16 => Ok(Self::Person),
            x if x == Self::Place as i16 => Ok(Self::Place),
            x if x == Self::Event as i16 => Ok(Self::Event),
            x if x == Self::Object as i16 => Ok(Self::Object),
            x if x == Self::Concept as i16 => Ok(Self::Concept),
            x if x == Self::Organization as i16 => Ok(Self::Organization),
            x if x == Self::Activity as i16 => Ok(Self::Activity),
            x if x == Self::DateTime as i16 => Ok(Self::DateTime),
            _ => Err(()),
        }
    }
}

impl EntityType {
    /// Wire representation of the entity type.
    ///
    /// The HTTP API and the LLM-facing tools carry entity types as these
    /// strings, so this is the single source of truth for the wire contract —
    /// independent of the derived `Debug` repr (issue #358).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Person => "Person",
            Self::Place => "Place",
            Self::Event => "Event",
            Self::Object => "Object",
            Self::Concept => "Concept",
            Self::Organization => "Organization",
            Self::Activity => "Activity",
            Self::DateTime => "DateTime",
        }
    }
}

impl std::str::FromStr for EntityType {
    type Err = ();

    /// Parse an entity-type wire string back into the enum.
    ///
    /// Mirrors [`EntityType::as_str`] case-insensitively, matching both the
    /// `kg_search` filter input and the LLM extraction validation contracts
    /// (issue #358).
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        ENTITY_TYPES
            .into_iter()
            .find(|ty| ty.as_str().eq_ignore_ascii_case(s))
            .ok_or(())
    }
}

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

    #[test]
    fn entity_type_try_from_roundtrip() {
        for id in 1..=8 {
            let ty = EntityType::try_from(id).unwrap();
            assert_eq!(ty as i16, id);
            assert_eq!(EntityType::try_from(ty as i16), Ok(ty));
        }
        assert!(EntityType::try_from(0).is_err());
        assert!(EntityType::try_from(9).is_err());
    }

    #[test]
    fn entity_type_as_str_matches_wire_contract() {
        assert_eq!(EntityType::Person.as_str(), "Person");
        assert_eq!(EntityType::Place.as_str(), "Place");
        assert_eq!(EntityType::Event.as_str(), "Event");
        assert_eq!(EntityType::Object.as_str(), "Object");
        assert_eq!(EntityType::Concept.as_str(), "Concept");
        assert_eq!(EntityType::Organization.as_str(), "Organization");
        assert_eq!(EntityType::Activity.as_str(), "Activity");
        assert_eq!(EntityType::DateTime.as_str(), "DateTime");
    }

    #[test]
    fn entity_type_from_str_roundtrip() {
        for ty in ENTITY_TYPES {
            assert_eq!(ty.as_str().parse(), Ok(ty));
            assert_eq!(ty.as_str().to_lowercase().parse(), Ok(ty));
        }
        assert!("bogus".parse::<EntityType>().is_err());
    }
}
