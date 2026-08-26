//! Sync staging: Graph delta results into the event buffer, `@removed`
//! deletions into the tombstone buffer, and event-to-fact conversion.

use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeZone, Utc};
use tracing::debug;

use crate::calendar::graph::GraphCalendarConnector;
use crate::calendar::graph::client::{
    GraphAttendee, GraphDateTime, GraphDeltaResult, GraphEvent, GraphRecurrencePattern,
    GraphRecurrenceRange,
};
use crate::connector::ConnectorError;
use crate::ical::{RawVEvent, vevent_to_facts};
use mimir_knowledge::normalize::NormalizedFact;

impl GraphCalendarConnector {
    /// Stage the changed events of a delta result into the buffer, returning
    /// the number of events staged.
    pub(super) async fn stage(&self, result: GraphDeltaResult) -> Result<u32, ConnectorError> {
        let count = result.events.len() as u32;
        let mut buffer = self.buffer.lock().await;
        for event in result.events {
            // Dedupe by Graph event id: a cycle cancelled after `sync` staged
            // events but before `extract` drained the buffer reuses the
            // previous cursor and stages the same delta again (issue #314),
            // so the pending buffer must keep one entry per event id — the
            // latest version wins — or `extract` would author one fact
            // cluster per buffered copy.
            if let Some(existing) = buffer.iter_mut().find(|e| e.id == event.id) {
                *existing = event;
            } else {
                buffer.push(event);
            }
        }
        drop(buffer);
        for id in result.deleted {
            // Server-side deletions (tombstones): the event id is the
            // `raw_reference` the extractor authors (see `event_to_facts`),
            // so staging it lets `extract_deletions` report the removal and
            // the supervisor trash exactly the facts this instance authored
            // for the deleted event (issue #247).
            let mut tombstones = self.tombstones.lock().await;
            // Dedupe: a failed cycle re-syncs from the last confirmed cursor
            // (issue #314), so the server re-reports the same deletions and
            // the pending buffer must not accumulate duplicates across
            // repeated failures (the trash path is idempotent, but the
            // buffer would grow unbounded).
            if !tombstones.contains(&id) {
                tombstones.push(id);
            }
        }
        Ok(count)
    }

    /// Convert one staged Graph event into its cluster of [`NormalizedFact`]s.
    ///
    /// Maps the event onto the shared [`RawVEvent`] shape and delegates to
    /// [`vevent_to_facts`] — the same fact cluster the CalDAV and iMIP
    /// backends author: `user has_event <event>` (typed
    /// [`mimir_knowledge::models::enums::EventType::Appointment`], recurrence from the
    /// Graph pattern), `<event> located_in <place>`, and
    /// `<attendee> attending <event>`. The `raw_reference` is the Graph
    /// event id, so an `@removed` tombstone maps 1:1 onto the authored
    /// facts.
    pub(super) fn event_to_facts(&self, event: &GraphEvent) -> Vec<NormalizedFact> {
        let vevent = graph_event_to_vevent(event);
        vevent_to_facts(self.user_identity.as_deref(), &vevent, &event.id)
    }
}

/// Map a Graph event onto the shared [`RawVEvent`] shape so the common
/// `vevent_to_facts` extractor authors the same fact cluster as the CalDAV
/// and iMIP backends (DRY).
fn graph_event_to_vevent(event: &GraphEvent) -> RawVEvent {
    RawVEvent {
        uid: Some(event.id.clone()),
        summary: event.subject.clone(),
        starts_at: event.start.as_ref().and_then(parse_graph_datetime),
        ends_at: event.end.as_ref().and_then(parse_graph_datetime),
        location: event
            .location
            .as_ref()
            .and_then(|l| l.display_name.as_deref())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        description: None,
        status: None,
        recurrence_rule: event.recurrence.as_ref().and_then(|r| {
            let pattern = r.pattern.as_ref()?;
            // The range's `startDate`/`endDate` boundaries are expressed in
            // `recurrenceTimeZone`, falling back to the event's own time zone
            // when absent (Microsoft Graph contract); the event time zone is
            // read from `start` first, then `end`.
            let event_time_zone = event
                .start
                .as_ref()
                .map(|s| s.time_zone.as_str())
                .or_else(|| event.end.as_ref().map(|e| e.time_zone.as_str()));
            graph_recurrence_to_rrule(pattern, r.range.as_ref(), event_time_zone)
        }),
        attendees: event
            .attendees
            .iter()
            .filter_map(attendee_display)
            .collect(),
        organizer: None,
    }
}

/// Parse a Graph `dateTime`/`timeZone` pair into UTC.
///
/// The `dateTime` is ISO 8601 local time expressed in the `timeZone` field
/// (IANA name; `UTC` by default). A `UTC` zone parses directly; any other
/// zone is resolved via `chrono-tz`. An unknown zone or an ambiguous local
/// time (rare DST fold) falls back to the naive value read as UTC so a bad
/// zone never silently drops the event — the same fallback the iCalendar
/// parser uses.
fn parse_graph_datetime(dt: &GraphDateTime) -> Option<DateTime<Utc>> {
    let naive = NaiveDateTime::parse_from_str(&dt.date_time, "%Y-%m-%dT%H:%M:%S%.f").ok()?;
    if dt.time_zone.eq_ignore_ascii_case("UTC") {
        return Some(Utc.from_utc_datetime(&naive));
    }
    if let Ok(tz) = dt.time_zone.parse::<chrono_tz::Tz>() {
        if let Some(local) = tz.from_local_datetime(&naive).single() {
            return Some(local.with_timezone(&Utc));
        }
    }
    debug!(time_zone = %dt.time_zone, "unknown Graph time zone; reading datetime as UTC");
    Some(Utc.from_utc_datetime(&naive))
}

/// Map a Graph recurrence pattern + range onto a full RRULE so the shared
/// `vevent_to_facts` recurrence mapping (which reads `RRULE` `FREQ`) can
/// advance recurring events. The interval, day/month constraints, and series
/// bounds (`COUNT` / `UNTIL`) are preserved so a fortnightly event stays
/// fortnightly and a bounded series stops advancing. `singleInstance` and
/// unknown types map to `None` (no recurrence).
fn graph_recurrence_to_rrule(
    pattern: &GraphRecurrencePattern,
    range: Option<&GraphRecurrenceRange>,
    event_time_zone: Option<&str>,
) -> Option<String> {
    let pattern_type = pattern.pattern_type.as_deref()?;
    let freq = match pattern_type.to_ascii_lowercase().as_str() {
        "daily" => "DAILY",
        "weekly" => "WEEKLY",
        "absolutemonthly" | "relativemonthly" => "MONTHLY",
        "absoluteyearly" | "relativeyearly" => "YEARLY",
        _ => return None,
    };
    let mut parts = vec![format!("FREQ={freq}")];
    if let Some(interval) = pattern.interval.filter(|i| *i > 1) {
        parts.push(format!("INTERVAL={interval}"));
    }
    match pattern_type.to_ascii_lowercase().as_str() {
        "weekly" => {
            if let Some(days) = pattern.days_of_week.as_deref().filter(|d| !d.is_empty()) {
                let byday = days
                    .iter()
                    .filter_map(|d| day_of_week_rrule(d))
                    .collect::<Vec<_>>()
                    .join(",");
                if !byday.is_empty() {
                    parts.push(format!("BYDAY={byday}"));
                }
            }
        }
        "absolutemonthly" => {
            if let Some(day) = pattern.day_of_month {
                parts.push(format!("BYMONTHDAY={day}"));
            }
        }
        "absoluteyearly" => {
            if let Some(month) = pattern.month {
                parts.push(format!("BYMONTH={month}"));
            }
            if let Some(day) = pattern.day_of_month {
                parts.push(format!("BYMONTHDAY={day}"));
            }
        }
        "relativemonthly" | "relativeyearly" => {
            if let Some(index) = pattern.index.as_deref().and_then(relative_index_rrule) {
                parts.push(format!("BYSETPOS={index}"));
            }
            if let Some(days) = pattern.days_of_week.as_deref().filter(|d| !d.is_empty()) {
                let byday = days
                    .iter()
                    .filter_map(|d| day_of_week_rrule(d))
                    .collect::<Vec<_>>()
                    .join(",");
                if !byday.is_empty() {
                    parts.push(format!("BYDAY={byday}"));
                }
            }
            if pattern_type.eq_ignore_ascii_case("relativeyearly") {
                if let Some(month) = pattern.month {
                    parts.push(format!("BYMONTH={month}"));
                }
            }
        }
        _ => {}
    }
    if let Some(range) = range {
        match range
            .range_type
            .as_deref()
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("numbered") => {
                if let Some(count) = range.number_of_occurrences.filter(|n| *n > 0) {
                    parts.push(format!("COUNT={count}"));
                }
            }
            Some("enddate") => {
                // The end date is an inclusive local boundary in the range's
                // time zone (falling back to the event time zone, then UTC),
                // so a zone ahead of UTC does not leak the next local date
                // into the series and a zone behind UTC does not truncate
                // the last local day.
                let time_zone = range.recurrence_time_zone.as_deref().or(event_time_zone);
                if let Some(until) = range
                    .end_date
                    .as_deref()
                    .and_then(|d| graph_end_date_to_until(d, time_zone))
                {
                    parts.push(format!("UNTIL={until}"));
                }
            }
            _ => {}
        }
    }
    Some(parts.join(";"))
}

/// Map a Graph `daysOfWeek` value onto its RRULE `BYDAY` two-letter code
/// (`sunday` → `SU`); unknown values map to `None`.
fn day_of_week_rrule(day: &str) -> Option<String> {
    let code = match day.trim().to_ascii_lowercase().as_str() {
        "sunday" => "SU",
        "monday" => "MO",
        "tuesday" => "TU",
        "wednesday" => "WE",
        "thursday" => "TH",
        "friday" => "FR",
        "saturday" => "SA",
        _ => return None,
    };
    Some(code.to_string())
}

/// Map a Graph relative `index` onto its RRULE `BYSETPOS` value (`first` →
/// `1`, `last` → `-1`); unknown values map to `None`.
fn relative_index_rrule(index: &str) -> Option<i32> {
    match index.trim().to_ascii_lowercase().as_str() {
        "first" => Some(1),
        "second" => Some(2),
        "third" => Some(3),
        "fourth" => Some(4),
        "last" => Some(-1),
        _ => None,
    }
}

/// Convert a Graph `endDate` (`YYYY-MM-DD`) into an RRULE `UNTIL` at the end
/// of that day, so occurrences on the end date are included. The boundary is
/// the inclusive local end-of-day (`23:59:59`) in `time_zone` (the range's
/// `recurrenceTimeZone`, else the event time zone, else UTC), converted to
/// UTC — a zone ahead of UTC must not leak the next local date into the
/// series, and a zone behind UTC must not truncate the last local day.
fn graph_end_date_to_until(end_date: &str, time_zone: Option<&str>) -> Option<String> {
    let date = NaiveDate::parse_from_str(end_date.trim(), "%Y-%m-%d").ok()?;
    let end_of_day = date.and_hms_opt(23, 59, 59)?;
    let utc = match time_zone {
        Some(tz) if !tz.eq_ignore_ascii_case("UTC") => {
            if let Ok(tz) = tz.parse::<chrono_tz::Tz>() {
                tz.from_local_datetime(&end_of_day)
                    .single()
                    .map(|local| local.with_timezone(&Utc))
            } else {
                // Unknown zone: fall back to reading the boundary as UTC so
                // a bad zone never drops the series bound (same fallback as
                // `parse_graph_datetime`).
                Some(Utc.from_utc_datetime(&end_of_day))
            }
        }
        _ => Some(Utc.from_utc_datetime(&end_of_day)),
    };
    utc.map(|dt| format!("{}Z", dt.format("%Y%m%dT%H%M%S")))
}

/// Resolve an attendee's display name (the address-book `name`, else the
/// SMTP `address`), skipping empty entries.
fn attendee_display(attendee: &GraphAttendee) -> Option<String> {
    let name = attendee
        .email_address
        .name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let address = attendee
        .email_address
        .address
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    name.or(address).map(str::to_string)
}
