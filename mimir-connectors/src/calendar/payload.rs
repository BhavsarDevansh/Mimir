//! Write-back payloads and iCalendar generation (C4 / #198).

use chrono::Utc;
use serde::Deserialize;

use crate::connector::ConnectorError;

/// Payload for a `create_event` / `update_event` write-back action.
///
/// `start`/`end` are RFC-3339 datetimes. `attendees` are bare addresses
/// (an optional `mailto:` prefix is normalised). `uid`/`href`/`etag` apply to
/// `update_event` (and may be supplied to `create_event` to pin the id).
#[derive(Debug, Deserialize)]
pub(super) struct WriteEventPayload {
    pub(super) summary: String,
    pub(super) start: String,
    #[serde(default)]
    pub(super) end: Option<String>,
    #[serde(default)]
    pub(super) location: Option<String>,
    #[serde(default)]
    pub(super) description: Option<String>,
    #[serde(default)]
    pub(super) attendees: Vec<String>,
    #[serde(default)]
    pub(super) uid: Option<String>,
    #[serde(default)]
    pub(super) href: Option<String>,
    #[serde(default)]
    pub(super) etag: Option<String>,
}

/// Payload for a `delete_event` write-back action.
#[derive(Debug, Deserialize)]
pub(super) struct DeleteEventPayload {
    pub(super) href: String,
    #[serde(default)]
    pub(super) etag: Option<String>,
}

/// Parse an RFC-3339 datetime into UTC, returning `None` on failure.
pub(super) fn parse_rfc3339(s: &str) -> Option<chrono::DateTime<Utc>> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

/// Build a `VCALENDAR`/`VEVENT` payload for a write-back `PUT`.
///
/// `uid` is the stable CalDAV item id (the href is `<calendar>/<uid>.ics`).
/// Empty optional fields are omitted so the emitted iCalendar stays minimal.
pub(super) fn build_vevent(
    payload: &WriteEventPayload,
    uid: &str,
) -> Result<String, ConnectorError> {
    use icalendar::{Calendar, Component, Event, EventLike};
    let start = parse_rfc3339(&payload.start).ok_or_else(|| {
        ConnectorError::Config(format!("invalid `start` datetime: {}", payload.start))
    })?;
    let mut event = Event::new();
    event
        .summary(payload.summary.trim())
        .uid(uid)
        .timestamp(Utc::now())
        .starts(start);
    if let Some(end_s) = payload.end.as_deref() {
        let end = parse_rfc3339(end_s)
            .ok_or_else(|| ConnectorError::Config(format!("invalid `end` datetime: {end_s}")))?;
        event.ends(end);
    }
    if let Some(loc) = payload
        .location
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        event.location(loc);
    }
    if let Some(desc) = payload
        .description
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        event.description(desc);
    }
    for attendee in &payload.attendees {
        let mail = attendee.trim();
        if mail.is_empty() {
            continue;
        }
        let mail = mail.strip_prefix("mailto:").unwrap_or(mail);
        event.add_multi_property("ATTENDEE", &format!("mailto:{mail}"));
    }
    let event = event.done();
    let mut calendar = Calendar::new();
    calendar.push(event);
    let calendar = calendar.done();
    Ok(calendar.to_string())
}
