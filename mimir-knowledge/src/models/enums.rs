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

/// Every [`RecurrenceType`] variant in discriminant order.
///
/// Single source of truth for callers that must enumerate the variants (for
/// example, to derive a JSON Schema `enum` from the serde representation
/// instead of re-typing the variant names). Keep this in lock-step with the
/// enum: every variant appears exactly once.
pub const RECURRENCE_TYPES: [RecurrenceType; 5] = [
    RecurrenceType::None,
    RecurrenceType::Daily,
    RecurrenceType::Weekly,
    RecurrenceType::Monthly,
    RecurrenceType::Yearly,
];

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
    /// A `Place` entity's own geographic coordinates (Phase 3 C2 / #196).
    /// Distinct from the person-location types above: a place does not
    /// "visit" a location, it *is* one. Used by the location-overlay worker
    /// to anchor a place entity created from a photo's GPS so `find_nearby`
    /// can resolve places by coordinates.
    Geographic = 6,
}

impl TryFrom<i16> for LocationType {
    type Error = ();

    fn try_from(value: i16) -> Result<Self, Self::Error> {
        match value {
            x if x == Self::Home as i16 => Ok(Self::Home),
            x if x == Self::Work as i16 => Ok(Self::Work),
            x if x == Self::Visited as i16 => Ok(Self::Visited),
            x if x == Self::Origin as i16 => Ok(Self::Origin),
            x if x == Self::Current as i16 => Ok(Self::Current),
            x if x == Self::Geographic as i16 => Ok(Self::Geographic),
            _ => Err(()),
        }
    }
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
// Lock the `Geographic` discriminant: the partial unique index and the
// `ON CONFLICT ... WHERE location_type_id = 6` upsert in `ensure_place_coordinates`
// hardcode `6` in SQL, so a drift here would silently break place anchoring.
const_assert!((LocationType::Geographic as i16) == 6);
const_assert!((DedupStatus::Pending as i16) != 0);
const_assert!((MergeWorkflowStatus::Pending as i16) != 0);
const_assert!((MergeResolution::Merged as i16) != 0);

/// External service connectors that extract facts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Type, serde::Serialize, serde::Deserialize)]
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

impl TryFrom<i16> for ConnectorType {
    type Error = ();

    fn try_from(value: i16) -> Result<Self, ()> {
        match value {
            x if x == Self::Gmail as i16 => Ok(Self::Gmail),
            x if x == Self::Calendar as i16 => Ok(Self::Calendar),
            x if x == Self::Photos as i16 => Ok(Self::Photos),
            x if x == Self::LinkedIn as i16 => Ok(Self::LinkedIn),
            _ => Err(()),
        }
    }
}

impl ConnectorType {
    /// Lowercase wire representation of the connector type.
    ///
    /// The HTTP API (`mimir-api-types`) carries connector types as strings,
    /// so this is the single source of truth for the wire contract —
    /// independent of the derived `Debug` repr (issue #264).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Gmail => "gmail",
            Self::Calendar => "calendar",
            Self::Photos => "photos",
            Self::LinkedIn => "linkedin",
        }
    }
}

impl std::str::FromStr for ConnectorType {
    type Err = ();

    /// Parse a lowercase wire `connector_type` string back into the enum.
    ///
    /// Mirrors [`ConnectorType::as_str`] so the input and output directions
    /// share one string table (issue #264). Returns `Err(())` for an unknown
    /// kind so the caller can surface a `400 Bad Request`.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "gmail" => Ok(Self::Gmail),
            "calendar" => Ok(Self::Calendar),
            "photos" => Ok(Self::Photos),
            "linkedin" => Ok(Self::LinkedIn),
            _ => Err(()),
        }
    }
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

impl ConnectorStatus {
    /// Lowercase wire representation of the connector lifecycle status.
    ///
    /// The HTTP API (`mimir-api-types`) carries statuses as strings, so this
    /// is the single source of truth for the wire contract — independent of
    /// the derived `Debug` repr (issue #264).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Setup => "setup",
            Self::Active => "active",
            Self::Paused => "paused",
            Self::Error => "error",
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

impl ConnectorAuthState {
    /// Lowercase wire representation of the connector auth state.
    ///
    /// The HTTP API (`mimir-api-types`) carries auth states as strings, so
    /// this is the single source of truth for the wire contract — independent
    /// of the derived `Debug` repr (issue #264).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unauthenticated => "unauthenticated",
            Self::Authenticated => "authenticated",
            Self::Expired => "expired",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

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
    fn recurrence_types_lists_every_variant_in_order() {
        // Single source of truth: the const array must enumerate the enum's
        // variants in discriminant order and stay in lock-step with it.
        assert_eq!(RECURRENCE_TYPES.len(), 5);
        assert_eq!(RECURRENCE_TYPES[0], RecurrenceType::None);
        assert_eq!(RECURRENCE_TYPES[1], RecurrenceType::Daily);
        assert_eq!(RECURRENCE_TYPES[2], RecurrenceType::Weekly);
        assert_eq!(RECURRENCE_TYPES[3], RecurrenceType::Monthly);
        assert_eq!(RECURRENCE_TYPES[4], RecurrenceType::Yearly);
        let mut sorted = RECURRENCE_TYPES;
        sorted.sort_by_key(|r| *r as i16);
        assert_eq!(sorted, RECURRENCE_TYPES);
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
    fn location_type_try_from_roundtrip() {
        assert_eq!(LocationType::try_from(1), Ok(LocationType::Home));
        assert_eq!(LocationType::try_from(2), Ok(LocationType::Work));
        assert_eq!(LocationType::try_from(3), Ok(LocationType::Visited));
        assert_eq!(LocationType::try_from(4), Ok(LocationType::Origin));
        assert_eq!(LocationType::try_from(5), Ok(LocationType::Current));
        assert_eq!(LocationType::try_from(6), Ok(LocationType::Geographic));
        assert_eq!(LocationType::try_from(0), Err(()));
        assert_eq!(LocationType::try_from(99), Err(()));
        assert_eq!(LocationType::try_from(-1), Err(()));
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

    #[test]
    fn connector_type_as_str_matches_wire_contract() {
        assert_eq!(ConnectorType::Gmail.as_str(), "gmail");
        assert_eq!(ConnectorType::Calendar.as_str(), "calendar");
        assert_eq!(ConnectorType::Photos.as_str(), "photos");
        assert_eq!(ConnectorType::LinkedIn.as_str(), "linkedin");
    }

    #[test]
    fn connector_type_from_str_roundtrips_wire_contract() {
        for t in [
            ConnectorType::Gmail,
            ConnectorType::Calendar,
            ConnectorType::Photos,
            ConnectorType::LinkedIn,
        ] {
            assert_eq!(ConnectorType::from_str(t.as_str()), Ok(t));
        }
        assert_eq!(ConnectorType::from_str("rss"), Err(()));
        assert_eq!(ConnectorType::from_str(""), Err(()));
    }

    #[test]
    fn connector_status_as_str_matches_wire_contract() {
        assert_eq!(ConnectorStatus::Setup.as_str(), "setup");
        assert_eq!(ConnectorStatus::Active.as_str(), "active");
        assert_eq!(ConnectorStatus::Paused.as_str(), "paused");
        assert_eq!(ConnectorStatus::Error.as_str(), "error");
    }

    #[test]
    fn connector_auth_state_as_str_matches_wire_contract() {
        assert_eq!(
            ConnectorAuthState::Unauthenticated.as_str(),
            "unauthenticated"
        );
        assert_eq!(ConnectorAuthState::Authenticated.as_str(), "authenticated");
        assert_eq!(ConnectorAuthState::Expired.as_str(), "expired");
    }
}

const_assert!((ConnectorType::Gmail as i16) != 0);

const_assert!((ConnectorStatus::Setup as i16) != 0);
const_assert!((ConnectorAuthState::Unauthenticated as i16) != 0);
