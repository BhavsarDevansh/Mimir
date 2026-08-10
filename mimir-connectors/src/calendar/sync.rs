//! Sync staging: parsed CalDAV resources into the event buffer and
//! event-to-fact conversion.

use tracing::{debug, warn};

use crate::calendar::CalendarConnector;
use crate::calendar::caldav::{RawCalDavEvent, SyncCollectionResult, parse_icalendar};
use crate::connector::ConnectorError;
use mimir_knowledge::normalize::NormalizedFact;

impl CalendarConnector {
    /// Stage the changed resources of a sync result into the buffer (parsed),
    /// returning the number of VEVENTs staged.
    pub(super) async fn stage(&self, result: SyncCollectionResult) -> Result<u32, ConnectorError> {
        let mut count = 0u32;
        for res in result.changed {
            if let Some(ical) = &res.calendar_data {
                let events = parse_icalendar(ical, &res.href, res.etag.as_deref());
                if events.is_empty() {
                    warn!(href = %res.href, "CalDAV resource had no parseable VEVENT");
                }
                count = count.saturating_add(events.len() as u32);
                self.buffer.lock().await.extend(events);
            } else {
                debug!(href = %res.href, "CalDAV changed resource had no calendar-data; skipping");
            }
        }
        for href in &result.deleted {
            // Server-side deletions (tombstones) are logged but not yet
            // propagated to the KB: surfacing a deletion needs a way for the
            // connector to report removals (extract only yields facts), so
            // trashing the corresponding facts is tracked as a follow-up.
            debug!(href = %href, "CalDAV reports deleted event; fact lifecycle deferred");
        }
        Ok(count)
    }

    /// Convert one staged VEVENT into its cluster of [`NormalizedFact`]s.
    ///
    /// Emits up to three fact shapes, all resolved by `normalize_and_insert`:
    /// 1. `user has_event <event>` — the primary appointment, carrying the
    ///    temporal bounds, the recurrence (`RRULE` `FREQ`), and an
    ///    [`EventType::Appointment`] hint so the events-subsystem overlay is
    ///    typed correctly. Authored only when a user identity is injected
    ///    (so the event surfaces in the user's "Upcoming" section); without
    ///    one the primary fact is skipped and the event is captured via its
    ///    location/attendee facts instead.
    /// 2. `<event> located_in <place>` — the `LOCATION` resolves to a
    ///    `Place` entity via the full F5 chain (no `entity_locations` overlay;
    ///    a calendar venue is a property of the event, not the user's
    ///    location history, so it does not bloat `Visited` rows). Carries no
    ///    temporal bounds, so it spawns no events-subsystem overlay.
    /// 3. `<attendee> attending <event>` — each `ATTENDEE` resolves to a
    ///    `Person` entity via F5. Like the location fact it carries no
    ///    temporal bounds and spawns no overlay.
    pub(super) fn event_to_facts(&self, event: &RawCalDavEvent) -> Vec<NormalizedFact> {
        // The VEVENT → fact cluster (`has_event` / `located_in` / `attending`)
        // is shared with the Email iMIP extraction in
        // [`crate::ical::vevent_to_facts`] (DRY). The CalDAV connector supplies
        // the user identity and the provenance `raw_reference` (the VEVENT UID,
        // falling back to the resource href) and delegates; entity resolution,
        // confidence, and the events-subsystem overlay run in the shared
        // `normalize_and_insert` pipeline.
        let raw_ref = event
            .vevent
            .uid
            .clone()
            .unwrap_or_else(|| event.href.clone());
        crate::ical::vevent_to_facts(self.user_identity.as_deref(), &event.vevent, &raw_ref)
    }

    /// Reject an event `href` that points outside the configured calendar
    /// collection, so a caller-supplied URL cannot redirect the stored
    /// credentials (Basic/Bearer auth, attached by `CalDavClient`) to another
    /// host or an unrelated resource. The check is origin-aware: the scheme,
    /// host, and port must match the configured `calendar_url`, and the path
    /// must lie under the calendar collection.
    pub(super) fn ensure_in_calendar(&self, href: &str) -> Result<(), ConnectorError> {
        let base = reqwest::Url::parse(self.config.calendar_url.trim_end_matches('/'))
            .map_err(|e| ConnectorError::Config(format!("invalid calendar_url: {e}")))?;
        let target = reqwest::Url::parse(href)
            .map_err(|e| ConnectorError::Config(format!("invalid event href `{href}`: {e}")))?;
        let same_origin = base.scheme() == target.scheme()
            && base.host_str() == target.host_str()
            && base.port() == target.port();
        if !same_origin {
            return Err(ConnectorError::Config(format!(
                "href `{href}` is outside the configured calendar origin"
            )));
        }
        let base_path = base.path().trim_end_matches('/');
        let under = base_path.is_empty()
            || target.path() == base_path
            || target.path().starts_with(&format!("{base_path}/"));
        if !under {
            return Err(ConnectorError::Config(format!(
                "href `{href}` is outside the configured calendar collection"
            )));
        }
        Ok(())
    }
}
