//! Preference model and related enums.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::Type;
use static_assertions::const_assert;

/// High-level category of a user preference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Type, Serialize, Deserialize)]
#[repr(i16)]
pub enum PreferenceCategory {
    CalendarBehavior = 1,
    NotificationStyle = 2,
    FoodPreference = 3,
    TravelPreference = 4,
    WorkStyle = 5,
    CommunicationPreference = 6,
    General = 7,
}

/// Every [`PreferenceCategory`] variant in discriminant order.
///
/// Single source of truth for callers that must enumerate the variants (for
/// example, the Obsidian format's category wire names, issue #62).
pub const PREFERENCE_CATEGORIES: [PreferenceCategory; 7] = [
    PreferenceCategory::CalendarBehavior,
    PreferenceCategory::NotificationStyle,
    PreferenceCategory::FoodPreference,
    PreferenceCategory::TravelPreference,
    PreferenceCategory::WorkStyle,
    PreferenceCategory::CommunicationPreference,
    PreferenceCategory::General,
];

impl PreferenceCategory {
    /// Wire representation of the preference category.
    ///
    /// Used by the Obsidian export/import format (issue #62) so the rendered
    /// `Category:` prefix and the parsed value share one string table.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CalendarBehavior => "CalendarBehavior",
            Self::NotificationStyle => "NotificationStyle",
            Self::FoodPreference => "FoodPreference",
            Self::TravelPreference => "TravelPreference",
            Self::WorkStyle => "WorkStyle",
            Self::CommunicationPreference => "CommunicationPreference",
            Self::General => "General",
        }
    }
}

impl std::str::FromStr for PreferenceCategory {
    type Err = ();

    /// Parse a category wire string back into the enum, case-insensitive.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        PREFERENCE_CATEGORIES
            .into_iter()
            .find(|c| c.as_str().eq_ignore_ascii_case(s.trim()))
            .ok_or(())
    }
}

impl TryFrom<i16> for PreferenceCategory {
    type Error = ();

    fn try_from(value: i16) -> Result<Self, Self::Error> {
        PREFERENCE_CATEGORIES
            .into_iter()
            .find(|c| *c as i16 == value)
            .ok_or(())
    }
}

/// How a preference value was determined.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Type, Serialize, Deserialize)]
#[repr(i16)]
pub enum PreferenceSourceType {
    Interaction = 1,
    Fact = 2,
    UserEdit = 3,
}

const_assert!((PreferenceCategory::CalendarBehavior as i16) != 0);
const_assert!((PreferenceSourceType::Interaction as i16) != 0);

/// A learned user preference with confidence and provenance.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow, Serialize, Deserialize)]
pub struct Preference {
    pub id: i32,
    pub entity_id: Option<i32>,
    pub category_id: i16,
    pub key: String,
    pub value: String,
    pub confidence: f32,
    pub overridden_by_user: bool,
    pub source_fact_id: Option<i32>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Normalized context condition for a preference.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow, Serialize, Deserialize)]
pub struct PreferenceContext {
    pub id: i32,
    pub preference_id: i32,
    pub context_key: String,
    pub context_value: String,
}

/// Provenance record linking a preference to its origin.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow, Serialize, Deserialize)]
pub struct PreferenceSource {
    pub id: i32,
    pub preference_id: i32,
    pub source_type_id: i16,
    pub source_id: String,
    pub extracted_at: DateTime<Utc>,
}

/// A single entry in the `preference_audit_log` table.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow, Serialize, Deserialize)]
pub struct PreferenceAuditLogEntry {
    pub id: i32,
    pub preference_id: i32,
    pub change_type_id: i16,
    pub old_value: Option<String>,
    pub new_value: Option<String>,
    pub changed_at: DateTime<Utc>,
    pub changed_by_id: Option<i16>,
    pub reason: Option<String>,
}

/// Input for inserting a new preference.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewPreference {
    pub entity_id: Option<i32>,
    pub category: PreferenceCategory,
    pub key: String,
    pub value: String,
    pub confidence: f32,
    pub overridden_by_user: bool,
    pub source_fact_id: Option<i32>,
}

/// Action taken during an upsert operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UpsertAction {
    Created,
    Overwritten,
    Rejected,
    KeptAsPrimary,
}

/// Full input for upserting a preference, including context and sources.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UpsertPreferenceInput {
    pub preference: NewPreference,
    pub changed_by: crate::models::audit_log::ChangedBy,
    pub contexts: Vec<(String, String)>,
    pub sources: Vec<(PreferenceSourceType, String)>,
}
