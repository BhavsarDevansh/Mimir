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
    Contradicts = 4,
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

impl TryFrom<i16> for RecurrenceType {
    type Error = ();

    fn try_from(value: i16) -> Result<Self, Self::Error> {
        match value {
            x if x == Self::None as i16 => Ok(Self::None),
            x if x == Self::Daily as i16 => Ok(Self::Daily),
            x if x == Self::Weekly as i16 => Ok(Self::Weekly),
            x if x == Self::Monthly as i16 => Ok(Self::Monthly),
            x if x == Self::Yearly as i16 => Ok(Self::Yearly),
            _ => Err(()),
        }
    }
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

/// Workflow status of a dedup queue entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Type, serde::Serialize, serde::Deserialize)]
#[repr(i16)]
pub enum DedupStatus {
    Pending = 1,
    Merged = 2,
    Kept = 3,
    Rejected = 4,
}

/// Workflow status of an entity merge queue entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Type, serde::Serialize, serde::Deserialize)]
#[repr(i16)]
pub enum MergeWorkflowStatus {
    Pending = 1,
    Processing = 2,
    Complete = 3,
}

/// Resolution outcome of an entity merge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Type, serde::Serialize, serde::Deserialize)]
#[repr(i16)]
pub enum MergeResolution {
    Merged = 1,
    KeptSeparate = 2,
    Rejected = 3,
}

// Compile-time sanity checks: max variant value fits i16 and no zero IDs.

// Verify no zero-ID variants.
const_assert!((RelationType::InferredFrom as i16) != 0);
const_assert!((EntityDateType::Birth as i16) != 0);
const_assert!((RecurrenceType::None as i16) != 0);
const_assert!((LocationType::Home as i16) != 0);
const_assert!((DedupStatus::Pending as i16) != 0);
const_assert!((MergeWorkflowStatus::Pending as i16) != 0);
const_assert!((MergeResolution::Merged as i16) != 0);

/// External service connectors that extract facts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Type, serde::Serialize, serde::Deserialize)]
#[repr(i16)]
pub enum ConnectorType {
    Gmail = 1,
    Calendar = 2,
    Photos = 3,
    LinkedIn = 4,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recurrence_type_try_from_valid() {
        assert_eq!(RecurrenceType::try_from(1), Ok(RecurrenceType::None));
        assert_eq!(RecurrenceType::try_from(2), Ok(RecurrenceType::Daily));
        assert_eq!(RecurrenceType::try_from(3), Ok(RecurrenceType::Weekly));
        assert_eq!(RecurrenceType::try_from(4), Ok(RecurrenceType::Monthly));
        assert_eq!(RecurrenceType::try_from(5), Ok(RecurrenceType::Yearly));
    }

    #[test]
    fn recurrence_type_try_from_rejects_unknown() {
        assert_eq!(RecurrenceType::try_from(0), Err(()));
        assert_eq!(RecurrenceType::try_from(6), Err(()));
        assert_eq!(RecurrenceType::try_from(-1), Err(()));
        assert_eq!(RecurrenceType::try_from(i16::MAX), Err(()));
    }

    #[test]
    fn recurrence_type_discriminant_values_are_stable() {
        // DB-stored IDs must remain stable across releases.
        assert_eq!(RecurrenceType::None as i16, 1);
        assert_eq!(RecurrenceType::Daily as i16, 2);
        assert_eq!(RecurrenceType::Weekly as i16, 3);
        assert_eq!(RecurrenceType::Monthly as i16, 4);
        assert_eq!(RecurrenceType::Yearly as i16, 5);
    }

    #[test]
    fn relation_type_discriminants_are_stable_and_nonzero() {
        assert_eq!(RelationType::InferredFrom as i16, 1);
        assert_eq!(RelationType::Corrects as i16, 2);
        assert_eq!(RelationType::Supersedes as i16, 3);
        assert_eq!(RelationType::Contradicts as i16, 4);
    }

    #[test]
    fn all_enum_discriminants_fit_i16_and_are_nonzero() {
        // Spot-check each enum's first variant to guard against accidental
        // renumbering that would corrupt existing DB rows.
        assert_ne!(EntityDateType::Birth as i16, 0);
        assert_ne!(LocationType::Home as i16, 0);
        assert_ne!(DedupStatus::Pending as i16, 0);
        assert_ne!(MergeWorkflowStatus::Pending as i16, 0);
        assert_ne!(MergeResolution::Merged as i16, 0);
        assert_ne!(ConnectorType::Gmail as i16, 0);
    }

    #[test]
    fn enums_serde_roundtrip() {
        fn rt<T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug>(
            v: T,
        ) {
            let json = serde_json::to_string(&v).unwrap();
            let back: T = serde_json::from_str(&json).unwrap();
            assert_eq!(back, v);
        }
        rt(RecurrenceType::Yearly);
        rt(RelationType::Contradicts);
        rt(ConnectorType::Calendar);
        rt(MergeResolution::KeptSeparate);
    }
}

const_assert!((ConnectorType::Gmail as i16) != 0);
