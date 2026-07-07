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

/// Kind of event tracked by the events/reminders subsystem.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Type, serde::Serialize, serde::Deserialize)]
#[repr(i16)]
pub enum EventType {
    Birthday = 1,
    Appointment = 2,
    Deadline = 3,
    Task = 4,
    Reminder = 5,
    Custom = 6,
}

impl TryFrom<i16> for EventType {
    type Error = ();

    fn try_from(value: i16) -> Result<Self, Self::Error> {
        match value {
            x if x == Self::Birthday as i16 => Ok(Self::Birthday),
            x if x == Self::Appointment as i16 => Ok(Self::Appointment),
            x if x == Self::Deadline as i16 => Ok(Self::Deadline),
            x if x == Self::Task as i16 => Ok(Self::Task),
            x if x == Self::Reminder as i16 => Ok(Self::Reminder),
            x if x == Self::Custom as i16 => Ok(Self::Custom),
            _ => Err(()),
        }
    }
}

/// Lifecycle status of an event overlay.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Type, serde::Serialize, serde::Deserialize)]
#[repr(i16)]
pub enum EventStatus {
    Pending = 1,
    Active = 2,
    Completed = 3,
    Dismissed = 4,
    Snoozed = 5,
}

impl TryFrom<i16> for EventStatus {
    type Error = ();

    fn try_from(value: i16) -> Result<Self, Self::Error> {
        match value {
            x if x == Self::Pending as i16 => Ok(Self::Pending),
            x if x == Self::Active as i16 => Ok(Self::Active),
            x if x == Self::Completed as i16 => Ok(Self::Completed),
            x if x == Self::Dismissed as i16 => Ok(Self::Dismissed),
            x if x == Self::Snoozed as i16 => Ok(Self::Snoozed),
            _ => Err(()),
        }
    }
}

/// Deterministic completion policy applied by the `events.upcoming_scan` job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Type, serde::Serialize, serde::Deserialize)]
#[repr(i16)]
pub enum AutoCompletePolicy {
    /// Transition to `Completed` once `trigger_date` has passed.
    AutoCompleteOnDate = 1,
    /// Stay `Active` until the user explicitly completes or dismisses.
    RequiresUserAction = 2,
    /// Never complete; advance `trigger_date` to the next recurrence.
    Recurring = 3,
}

impl TryFrom<i16> for AutoCompletePolicy {
    type Error = ();

    fn try_from(value: i16) -> Result<Self, Self::Error> {
        match value {
            x if x == Self::AutoCompleteOnDate as i16 => Ok(Self::AutoCompleteOnDate),
            x if x == Self::RequiresUserAction as i16 => Ok(Self::RequiresUserAction),
            x if x == Self::Recurring as i16 => Ok(Self::Recurring),
            _ => Err(()),
        }
    }
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
const_assert!((EventType::Birthday as i16) != 0);
const_assert!((EventStatus::Active as i16) != 0);
const_assert!((AutoCompletePolicy::AutoCompleteOnDate as i16) != 0);
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

/// Lifecycle status of a connector instance.
///
/// Stored as the `status_id` foreign key on the `connectors` table and mirrored
/// in the `connector_statuses` lookup table (migration 042).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Type, serde::Serialize, serde::Deserialize)]
#[repr(i16)]
pub enum ConnectorStatus {
    /// Initial state — configured but not yet authenticated or started.
    Setup = 1,
    /// Running and healthy; the supervisor may poll it.
    Active = 2,
    /// Manually paused by the user; not auto-restarted.
    Paused = 3,
    /// Circuit-breaker tripped after repeated failures; requires manual resume.
    Error = 4,
}

impl TryFrom<i16> for ConnectorStatus {
    type Error = ();

    fn try_from(value: i16) -> Result<Self, ()> {
        match value {
            x if x == Self::Setup as i16 => Ok(Self::Setup),
            x if x == Self::Active as i16 => Ok(Self::Active),
            x if x == Self::Paused as i16 => Ok(Self::Paused),
            x if x == Self::Error as i16 => Ok(Self::Error),
            _ => Err(()),
        }
    }
}

/// Authentication state of a connector instance.
///
/// Stored as the `auth_state_id` foreign key on the `connectors` table and
/// mirrored in the `connector_auth_states` lookup table (migration 042).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Type, serde::Serialize, serde::Deserialize)]
#[repr(i16)]
pub enum ConnectorAuthState {
    /// No credentials stored yet.
    Unauthenticated = 1,
    /// Valid credentials present; the connector may sync.
    Authenticated = 2,
    /// Token expired or revoked; re-authentication required.
    Expired = 3,
}

impl TryFrom<i16> for ConnectorAuthState {
    type Error = ();

    fn try_from(value: i16) -> Result<Self, Self::Error> {
        match value {
            x if x == Self::Unauthenticated as i16 => Ok(Self::Unauthenticated),
            x if x == Self::Authenticated as i16 => Ok(Self::Authenticated),
            x if x == Self::Expired as i16 => Ok(Self::Expired),
            _ => Err(()),
        }
    }
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
    fn event_type_discriminants_are_stable() {
        assert_eq!(EventType::Birthday as i16, 1);
        assert_eq!(EventType::Appointment as i16, 2);
        assert_eq!(EventType::Deadline as i16, 3);
        assert_eq!(EventType::Task as i16, 4);
        assert_eq!(EventType::Reminder as i16, 5);
        assert_eq!(EventType::Custom as i16, 6);
    }

    #[test]
    fn event_status_discriminants_are_stable() {
        assert_eq!(EventStatus::Pending as i16, 1);
        assert_eq!(EventStatus::Active as i16, 2);
        assert_eq!(EventStatus::Completed as i16, 3);
        assert_eq!(EventStatus::Dismissed as i16, 4);
        assert_eq!(EventStatus::Snoozed as i16, 5);
    }

    #[test]
    fn auto_complete_policy_discriminants_are_stable() {
        assert_eq!(AutoCompletePolicy::AutoCompleteOnDate as i16, 1);
        assert_eq!(AutoCompletePolicy::RequiresUserAction as i16, 2);
        assert_eq!(AutoCompletePolicy::Recurring as i16, 3);
    }

    #[test]
    fn event_enums_try_from_roundtrip() {
        assert_eq!(EventType::try_from(4), Ok(EventType::Task));
        assert_eq!(EventStatus::try_from(3), Ok(EventStatus::Completed));
        assert_eq!(
            AutoCompletePolicy::try_from(2),
            Ok(AutoCompletePolicy::RequiresUserAction)
        );
        assert_eq!(EventType::try_from(0), Err(()));
        assert_eq!(EventStatus::try_from(99), Err(()));
        assert_eq!(AutoCompletePolicy::try_from(-1), Err(()));
    }

    #[test]
    fn all_enum_discriminants_fit_i16_and_are_nonzero() {
        // Spot-check each enum's first variant to guard against accidental
        // renumbering that would corrupt existing DB rows.
        assert_ne!(EventType::Birthday as i16, 0);
        assert_ne!(EventStatus::Active as i16, 0);
        assert_ne!(AutoCompletePolicy::AutoCompleteOnDate as i16, 0);
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
        rt(EventType::Deadline);
        rt(EventStatus::Snoozed);
        rt(AutoCompletePolicy::Recurring);
        rt(ConnectorType::Calendar);
        rt(MergeResolution::KeptSeparate);
    }

    #[test]
    fn connector_status_discriminants_are_stable() {
        assert_eq!(ConnectorStatus::Setup as i16, 1);
        assert_eq!(ConnectorStatus::Active as i16, 2);
        assert_eq!(ConnectorStatus::Paused as i16, 3);
        assert_eq!(ConnectorStatus::Error as i16, 4);
    }

    #[test]
    fn connector_auth_state_discriminants_are_stable() {
        assert_eq!(ConnectorAuthState::Unauthenticated as i16, 1);
        assert_eq!(ConnectorAuthState::Authenticated as i16, 2);
        assert_eq!(ConnectorAuthState::Expired as i16, 3);
    }

    #[test]
    fn connector_enums_try_from_roundtrip() {
        assert_eq!(ConnectorStatus::try_from(3), Ok(ConnectorStatus::Paused));
        assert_eq!(
            ConnectorAuthState::try_from(2),
            Ok(ConnectorAuthState::Authenticated)
        );
        assert_eq!(ConnectorStatus::try_from(0), Err(()));
        assert_eq!(ConnectorAuthState::try_from(9), Err(()));
    }
}

const_assert!((ConnectorType::Gmail as i16) != 0);

const_assert!((ConnectorStatus::Setup as i16) != 0);
const_assert!((ConnectorAuthState::Unauthenticated as i16) != 0);
