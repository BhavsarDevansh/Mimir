//! Preference model and related enums.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::Type;
use static_assertions::const_assert;

/// High-level category of a user preference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Type, Serialize, Deserialize)]
#[repr(i16)]
pub enum PreferenceCategory {
    NotificationStyle = 1,
    CalendarAutoAdd = 2,
    ProactivityLevel = 3,
    CommunicationTone = 4,
    Privacy = 5,
}

/// How a preference value was determined.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Type, Serialize, Deserialize)]
#[repr(i16)]
pub enum PreferenceSourceType {
    Explicit = 1,
    Inferred = 2,
    Corrected = 3,
}

const_assert!((PreferenceCategory::NotificationStyle as i16) != 0);
const_assert!((PreferenceSourceType::Explicit as i16) != 0);

/// A learned user preference with confidence and provenance.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow, Serialize, Deserialize)]
pub struct Preference {
    pub id: i32,
    pub category_id: i16,
    pub key: String,
    pub value: String, // JSON
    pub confidence: f32,
    pub overridden_by_user: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
