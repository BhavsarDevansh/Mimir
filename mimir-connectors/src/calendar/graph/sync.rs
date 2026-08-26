//! Sync staging: Graph delta results into the event buffer, `@removed`
//! deletions into the tombstone buffer, and event-to-fact conversion.

use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
use tracing::debug;

use crate::calendar::graph::GraphCalendarConnector;
use crate::calendar::graph::client::{GraphAttendee, GraphDateTime, GraphDeltaResult, GraphEvent};
use crate::connector::ConnectorError;
use crate::ical::{RawVEvent, vevent_to_facts};
use mimir_knowledge::normalize::NormalizedFact;

impl GraphCalendarConnector {
    /// Stage the changed events of a delta result into the buffer, returning
    /// the number of events staged.
    pub(super) async fn stage(&self, result: GraphDeltaResult) -> Result<u32, ConnectorError> {
        let count = result.events.len() as u32;
        self.buffer.lock().await.extend(result.events);
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
        recurrence_rule: event
            .recurrence
            .as_ref()
            .and_then(|r| r.pattern.as_ref())
            .and_then(|p| graph_recurrence_to_rrule(p.pattern_type.as_deref())),
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

/// Map a Graph recurrence pattern type onto an RRULE `FREQ` so the shared
/// `vevent_to_facts` recurrence mapping (which reads `RRULE` `FREQ`) can
/// advance recurring events. `singleInstance` and unknown types map to
/// `None` (no recurrence).
fn graph_recurrence_to_rrule(pattern_type: Option<&str>) -> Option<String> {
    let freq = match pattern_type?.to_ascii_lowercase().as_str() {
        "daily" => "DAILY",
        "weekly" => "WEEKLY",
        "absolutemonthly" | "relativemonthly" => "MONTHLY",
        "absoluteyearly" | "relativeyearly" => "YEARLY",
        _ => return None,
    };
    Some(format!("FREQ={freq}"))
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
