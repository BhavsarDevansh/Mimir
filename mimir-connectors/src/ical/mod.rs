//! Shared iCalendar VEVENT parsing + fact extraction (Phase 3 C4 / #198 and
//! C6 / #200).
//!
//! Both connector backends that consume iCalendar data — the CalDAV Calendar
//! connector (`#197`/`#198`) and the IMAP Email connector's iMIP-invite
//! extraction (`#200`) — parse the *same* RFC 5545 VEVENT payload into the
//! *same* intrinsic fields and turn it into the *same* cluster of
//! [`mimir_knowledge::normalize::NormalizedFact`]s. This module is the single source of truth for both:
//!
//! - [`crate::ical::parse_ical_to_vevents`] parses an iCalendar text payload into one
//!   [`crate::ical::RawVEvent`] per `VEVENT` (UTC-resolved dates, resolved attendee display
//!   names, raw `RRULE`). It does not know where the payload came from
//!   (a CalDAV resource or a `text/calendar` MIME part) — that provenance is
//!   supplied by the caller as the `raw_reference` on each fact.
//! - [`crate::ical::vevent_to_facts`] turns one [`crate::ical::RawVEvent`] into the appointment cluster:
//!   `user has_event <event>` (typed [`mimir_knowledge::models::enums::EventType::Appointment`], recurrence
//!   from `RRULE` `FREQ`), `<event> located_in <place>`, and
//!   `<attendee> attending <event>`. Entity resolution, confidence, and the
//!   events-subsystem overlay are left to the shared `normalize_and_insert`
//!   pipeline; this function only builds the typed facts.
//!
//! Gated by `any(feature = "calendar", feature = "email")`: it is needed only
//! by the two connector backends that consume iCalendar. The parsing helpers
//! depend on `icalendar` (the `parser` submodule) and `chrono-tz`, both made
//! available to either backend by their feature flags.

use chrono::{DateTime, Utc};

// ---------------------------------------------------------------------------
// Intrinsic parsed VEVENT
// ---------------------------------------------------------------------------

/// A parsed iCalendar `VEVENT`'s intrinsic fields — the data both the CalDAV
/// Calendar connector and the IMAP Email iMIP extraction need.
///
/// Dates are parsed to UTC at staging time: `DTSTART`/`DTEND` may be UTC,
/// floating local, date-only, or `TZID`-qualified; the latter is resolved via
/// `chrono-tz`. `attendees` carry resolved display names (the `CN` parameter,
/// else the `mailto:` value) so the fact extractor can resolve them to `Person`
/// entities via the full F5 chain. `recurrence_rule` stays a raw `RRULE`
/// string; [`vevent_to_facts`] maps `FREQ` to [`RecurrenceType`] so the
/// existing events-subsystem recurrence logic advances recurring events.
///
/// CalDAV-specific provenance (the resource `href` and `etag`) is *not*
/// intrinsic — the Calendar connector wraps this in a `RawCalDavEvent`
mod facts;
mod parse;
#[cfg(test)]
mod tests;

pub use facts::vevent_to_facts;
pub use parse::parse_ical_to_vevents;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawVEvent {
    /// `UID` property.
    pub uid: Option<String>,
    /// `SUMMARY` property.
    pub summary: Option<String>,
    /// Parsed `DTSTART` resolved to UTC. `None` when the value is absent or
    /// unparseable ([`vevent_to_facts`] skips events without a start).
    pub starts_at: Option<DateTime<Utc>>,
    /// Parsed `DTEND` resolved to UTC. `None` when absent (an all-day or
    /// unbounded event); the extractor treats the event as point-in-time.
    pub ends_at: Option<DateTime<Utc>>,
    /// `LOCATION` property.
    pub location: Option<String>,
    /// `DESCRIPTION` property.
    pub description: Option<String>,
    /// `STATUS` property (e.g. `CONFIRMED` / `CANCELLED`). Not used by fact
    /// extraction today; retained for a future CANCEL lifecycle / status pass.
    pub status: Option<String>,
    /// `RRULE` property (recurrence; the events-subsystem #74 owns this).
    pub recurrence_rule: Option<String>,
    /// Resolved display names of every `ATTENDEE` (the `CN` parameter, else
    /// the `mailto:` value), in document order.
    pub attendees: Vec<String>,
    /// Resolved display name of the `ORGANIZER`, if present.
    pub organizer: Option<String>,
}
