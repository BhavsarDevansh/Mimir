//! Audit log model with typed change_type and changed_by enums.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::Type;
use static_assertions::const_assert;

/// What happened to a fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Type, Serialize, Deserialize)]
#[repr(i16)]
pub enum ChangeType {
    Created = 1,
    StatusChange = 2,
    ConfidenceChange = 3,
    TemporalUpdate = 4,
    SourceAdded = 5,
    Forgotten = 6,
    Restored = 7,
    Rejected = 8,
    ContentUpdate = 9,
}

const_assert!((ChangeType::Created as i16) != 0);

/// Who or what triggered the change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Type, Serialize, Deserialize)]
#[repr(i16)]
pub enum ChangedBy {
    User = 1,
    System = 2,
    InferenceEngine = 3,
    NightlyOptimization = 4,
}

const_assert!((ChangedBy::User as i16) != 0);

/// A single entry in the `fact_audit_log` table.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow, Serialize, Deserialize)]
pub struct AuditLogEntry {
    pub id: i32,
    pub fact_id: i32,
    pub change_type_id: i16,
    pub old_value: Option<String>,
    pub new_value: Option<String>,
    pub changed_at: DateTime<Utc>,
    pub changed_by_id: Option<i16>,
    pub reason: Option<String>,
}

impl TryFrom<i16> for ChangeType {
    type Error = ();

    fn try_from(value: i16) -> Result<Self, Self::Error> {
        match value {
            x if x == Self::Created as i16 => Ok(Self::Created),
            x if x == Self::StatusChange as i16 => Ok(Self::StatusChange),
            x if x == Self::ConfidenceChange as i16 => Ok(Self::ConfidenceChange),
            x if x == Self::TemporalUpdate as i16 => Ok(Self::TemporalUpdate),
            x if x == Self::SourceAdded as i16 => Ok(Self::SourceAdded),
            x if x == Self::Forgotten as i16 => Ok(Self::Forgotten),
            x if x == Self::Restored as i16 => Ok(Self::Restored),
            x if x == Self::Rejected as i16 => Ok(Self::Rejected),
            x if x == Self::ContentUpdate as i16 => Ok(Self::ContentUpdate),
            _ => Err(()),
        }
    }
}

impl ChangeType {
    /// Wire representation of the change type.
    ///
    /// The HTTP API (`mimir-api-types`) and the `change_types` lookup table
    /// carry change types as these lowercase strings, so this is the single
    /// source of truth for the wire contract — independent of the derived
    /// `Debug` repr (issue #358).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::StatusChange => "status_change",
            Self::ConfidenceChange => "confidence_change",
            Self::TemporalUpdate => "temporal_update",
            Self::SourceAdded => "source_added",
            Self::Forgotten => "forgotten",
            Self::Restored => "restored",
            Self::Rejected => "rejected",
            Self::ContentUpdate => "content_update",
        }
    }
}

impl std::str::FromStr for ChangeType {
    type Err = ();

    /// Parse a wire `change_type` string back into the enum.
    ///
    /// Mirrors [`ChangeType::as_str`] case-insensitively, matching the
    /// historical `kb audit --change-type` input contract, so the input and
    /// output directions share one string table (issue #358).
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        [
            Self::Created,
            Self::StatusChange,
            Self::ConfidenceChange,
            Self::TemporalUpdate,
            Self::SourceAdded,
            Self::Forgotten,
            Self::Restored,
            Self::Rejected,
            Self::ContentUpdate,
        ]
        .into_iter()
        .find(|ty| ty.as_str().eq_ignore_ascii_case(s))
        .ok_or(())
    }
}

impl TryFrom<i16> for ChangedBy {
    type Error = ();

    fn try_from(value: i16) -> Result<Self, Self::Error> {
        match value {
            x if x == Self::User as i16 => Ok(Self::User),
            x if x == Self::System as i16 => Ok(Self::System),
            x if x == Self::InferenceEngine as i16 => Ok(Self::InferenceEngine),
            x if x == Self::NightlyOptimization as i16 => Ok(Self::NightlyOptimization),
            _ => Err(()),
        }
    }
}

impl ChangedBy {
    /// Wire representation of the change actor.
    ///
    /// The fact-detail HTTP API carries the changed-by actor as these
    /// variant-style strings, so this is the single source of truth for the
    /// wire contract — independent of the derived `Debug` repr (issue #358).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "User",
            Self::System => "System",
            Self::InferenceEngine => "InferenceEngine",
            Self::NightlyOptimization => "NightlyOptimization",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn change_type_try_from_roundtrip() {
        for id in 1..=9 {
            let ty = ChangeType::try_from(id).unwrap();
            assert_eq!(ty as i16, id);
            assert_eq!(ChangeType::try_from(ty as i16), Ok(ty));
        }
        assert!(ChangeType::try_from(0).is_err());
        assert!(ChangeType::try_from(10).is_err());
    }

    #[test]
    fn change_type_as_str_matches_wire_contract() {
        assert_eq!(ChangeType::Created.as_str(), "created");
        assert_eq!(ChangeType::StatusChange.as_str(), "status_change");
        assert_eq!(ChangeType::ConfidenceChange.as_str(), "confidence_change");
        assert_eq!(ChangeType::TemporalUpdate.as_str(), "temporal_update");
        assert_eq!(ChangeType::SourceAdded.as_str(), "source_added");
        assert_eq!(ChangeType::Forgotten.as_str(), "forgotten");
        assert_eq!(ChangeType::Restored.as_str(), "restored");
        assert_eq!(ChangeType::Rejected.as_str(), "rejected");
        assert_eq!(ChangeType::ContentUpdate.as_str(), "content_update");
    }

    #[test]
    fn change_type_from_str_roundtrip() {
        for ty in [
            ChangeType::Created,
            ChangeType::StatusChange,
            ChangeType::ConfidenceChange,
            ChangeType::TemporalUpdate,
            ChangeType::SourceAdded,
            ChangeType::Forgotten,
            ChangeType::Restored,
            ChangeType::Rejected,
            ChangeType::ContentUpdate,
        ] {
            assert_eq!(ChangeType::from_str(ty.as_str()), Ok(ty));
            assert_eq!(ChangeType::from_str(&ty.as_str().to_uppercase()), Ok(ty));
        }
        assert!(ChangeType::from_str("bogus").is_err());
    }

    #[test]
    fn changed_by_try_from_roundtrip() {
        for id in 1..=4 {
            let by = ChangedBy::try_from(id).unwrap();
            assert_eq!(by as i16, id);
            assert_eq!(ChangedBy::try_from(by as i16), Ok(by));
        }
        assert!(ChangedBy::try_from(0).is_err());
        assert!(ChangedBy::try_from(5).is_err());
    }

    #[test]
    fn changed_by_as_str_matches_wire_contract() {
        assert_eq!(ChangedBy::User.as_str(), "User");
        assert_eq!(ChangedBy::System.as_str(), "System");
        assert_eq!(ChangedBy::InferenceEngine.as_str(), "InferenceEngine");
        assert_eq!(
            ChangedBy::NightlyOptimization.as_str(),
            "NightlyOptimization"
        );
    }
}
