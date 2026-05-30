//! Shared lookup-table enums mapped to stable DB IDs via `#[repr(i16)]`.

use sqlx::Type;
use static_assertions::const_assert;

/// Types of relationships between facts in the dependency graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Type, serde::Serialize, serde::Deserialize)]
#[repr(i16)]
pub enum RelationType {
    InferredFrom = 1,
    Corrects = 2,
    Supersedes = 3,
}

/// Classification of dates associated with entities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Type, serde::Serialize, serde::Deserialize)]
#[repr(i16)]
pub enum EntityDateType {
    Birth = 1,
    Death = 2,
    Anniversary = 3,
    Created = 4,
    Dissolved = 5,
    Custom = 6,
}

/// How an entity date recurs (if at all).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Type, serde::Serialize, serde::Deserialize)]
#[repr(i16)]
pub enum RecurrenceType {
    None = 1,
    Daily = 2,
    Weekly = 3,
    Monthly = 4,
    Yearly = 5,
}

/// Classification of locations associated with entities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Type, serde::Serialize, serde::Deserialize)]
#[repr(i16)]
pub enum LocationType {
    Home = 1,
    Work = 2,
    Visited = 3,
    Origin = 4,
    Current = 5,
}

// Compile-time sanity checks: max variant value fits i16 and no zero IDs.

// Verify no zero-ID variants.
const_assert!((RelationType::InferredFrom as i16) != 0);
const_assert!((EntityDateType::Birth as i16) != 0);
const_assert!((RecurrenceType::None as i16) != 0);
const_assert!((LocationType::Home as i16) != 0);
