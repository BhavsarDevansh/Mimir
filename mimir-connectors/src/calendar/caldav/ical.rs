//! iCalendar payload decoding for CalDAV resources.
//!
//! Decodes raw `VCALENDAR` text into typed [`RawCalDavEvent`]s using the
//! `icalendar` crate, tolerating invalid payloads by returning an empty list.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawCalDavEvent {
    /// The shared intrinsic VEVENT fields (UID, SUMMARY, dates, location,
    /// description, status, attendees, organiser, RRULE).
    pub vevent: crate::ical::RawVEvent,
    /// The CalDAV resource href (the item id).
    pub href: String,
    /// ETag, if known.
    pub etag: Option<String>,
}

/// Parse an iCalendar payload into the CalDAV resources it contains.
///
/// Returns one [`RawCalDavEvent`] per `VEVENT`, wrapping the shared
/// [`crate::ical::RawVEvent`] with the CalDAV `href`/`etag`. An empty/invalid
/// payload yields an empty vec (the connector logs and skips rather than
/// failing the sync), so one malformed event never aborts a whole sync.
pub fn parse_icalendar(ical: &str, href: &str, etag: Option<&str>) -> Vec<RawCalDavEvent> {
    crate::ical::parse_ical_to_vevents(ical)
        .into_iter()
        .map(|vevent| RawCalDavEvent {
            vevent,
            href: href.to_string(),
            etag: etag.map(str::to_string),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
