//! iCalendar payload parsing: `VEVENT` extraction and date/participant
//! decoding.

use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeZone, Utc};

use crate::ical::RawVEvent;

pub fn parse_ical_to_vevents(ical: &str) -> Vec<RawVEvent> {
    // The low-level parser (`icalendar::parser`) yields a zero-copy
    // `Calendar` whose top-level `components` are the VEVENT/VTODO/VTIMEZONE
    // entries. We walk the low-level representation directly (the high-level
    // `icalendar::Calendar` is builder-oriented and has no parse-from-str
    // path in 0.17.x). We walk the low-level `Component::properties` directly
    // with a case-insensitive lookup (`Component::find_prop` matches names
    // case-sensitively in 0.17.x, but RFC 5545 names are case-insensitive) and
    // `ParseString::as_str` gives owned copies so the staged events outlive
    // the borrowed input; `params` expose the `TZID`/`CN` parameters the
    // extractors need for UTC resolution and attendee names.
    use icalendar::parser::read_calendar;
    let Ok(calendar) = read_calendar(ical) else {
        return Vec::new();
    };
    calendar
        .components
        .iter()
        .filter(|c| c.name.as_str().eq_ignore_ascii_case("VEVENT"))
        .map(|event| {
            // RFC 5545 property names are case-insensitive; `find_prop` in
            // `icalendar` 0.17.x matches case-sensitively, so look the property
            // up by a case-insensitive name over `Component::properties`.
            let find_prop_ci = |key: &str| {
                event
                    .properties
                    .iter()
                    .find(|p| p.name.as_ref().eq_ignore_ascii_case(key))
            };
            let prop_str = |key: &str| find_prop_ci(key).map(|p| p.val.as_str().to_string());
            // Read a property's `TZID` parameter (case-insensitive key).
            let tzid_of = |key: &str| {
                find_prop_ci(key).and_then(|p| {
                    p.params
                        .iter()
                        .find(|q| q.key.as_ref().eq_ignore_ascii_case("TZID"))
                        .and_then(|q| q.val.as_ref().map(|v| v.as_str().to_string()))
                })
            };
            let starts_at = find_prop_ci("DTSTART")
                .and_then(|p| parse_ical_datetime(p.val.as_str(), tzid_of("DTSTART").as_deref()));
            let ends_at = find_prop_ci("DTEND")
                .and_then(|p| parse_ical_datetime(p.val.as_str(), tzid_of("DTEND").as_deref()));
            let attendees = event
                .properties
                .iter()
                .filter(|p| p.name.as_ref().eq_ignore_ascii_case("ATTENDEE"))
                .filter_map(participant_display)
                .collect();
            let organizer = find_prop_ci("ORGANIZER").and_then(participant_display);
            RawVEvent {
                uid: prop_str("UID"),
                summary: prop_str("SUMMARY"),
                starts_at,
                ends_at,
                location: prop_str("LOCATION"),
                description: prop_str("DESCRIPTION"),
                status: prop_str("STATUS"),
                recurrence_rule: prop_str("RRULE"),
                attendees,
                organizer,
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// VEVENT → NormalizedFact cluster
// ---------------------------------------------------------------------------

/// Turn one parsed [`RawVEvent`] into its cluster of [`NormalizedFact`]s.
///
/// Emits up to three fact shapes, all resolved by the shared
/// `normalize_and_insert` pipeline:
/// 1. `user has_event <event>` — the primary appointment, carrying the
///    temporal bounds, the recurrence (`RRULE` `FREQ`), and an
///    [`EventType::Appointment`] hint so the events-subsystem overlay is
///    typed correctly. Authored only when a user identity is supplied (so the
///    event surfaces in the user's "Upcoming" section); without one the
///    primary fact is skipped and the event is captured via its
///    location/attendee facts instead.
/// 2. `<event> located_in <place>` — the `LOCATION` resolves to a `Place`
///    entity via the full F5 chain (no `entity_locations` overlay; a venue is
///    a property of the event, not the user's location history). Carries no
///    temporal bounds, so it spawns no events-subsystem overlay.
/// 3. `<attendee> attending <event>` — each `ATTENDEE` resolves to a `Person`
///    entity via F5. Like the location fact it carries no temporal bounds and
///    spawns no overlay.
///
/// `raw_reference` is the native id of the source item (a CalDAV VEVENT `UID`,
/// or an iMIP invite's email UID). It rides on every fact as the provenance
/// `raw_reference` so the KB can trace each fact back to its source.
pub(super) fn parse_ical_datetime(value: &str, tzid: Option<&str>) -> Option<DateTime<Utc>> {
    const NAIVE_FMT: &str = "%Y%m%dT%H%M%S";
    const DATE_FMT: &str = "%Y%m%d";

    // UTC form (`20250503T090000Z`): strip the trailing `Z`, parse the naive
    // datetime, and label it UTC. Avoids the deprecated
    // `TimeZone::datetime_from_str`.
    if let Some(rest) = value.strip_suffix('Z') {
        if let Ok(naive) = NaiveDateTime::parse_from_str(rest, NAIVE_FMT) {
            return Some(naive.and_utc());
        }
    }
    if let Some(tz) = tzid {
        if let Ok(naive) = NaiveDateTime::parse_from_str(value, NAIVE_FMT) {
            if let Ok(zone) = tz.parse::<chrono_tz::Tz>() {
                // An ambiguous autumn-fold local time resolves to a single
                // instant when unambiguous; otherwise prefer the earliest
                // offset (keeping the value within an hour of the wall clock)
                // before falling back to naive-as-UTC. A spring-forward gap
                // (`None` from both) still hits the fallback below.
                let local = zone
                    .from_local_datetime(&naive)
                    .single()
                    .or_else(|| zone.from_local_datetime(&naive).earliest());
                if let Some(local) = local {
                    return Some(local.with_timezone(&Utc));
                }
            }
            // Unknown zone or genuinely ambiguous/unrepresentable local time:
            // read the naive value as UTC so a bad `TZID` never drops the event.
            return Some(naive.and_utc());
        }
    }
    if let Ok(naive) = NaiveDateTime::parse_from_str(value, NAIVE_FMT) {
        return Some(naive.and_utc());
    }
    if let Ok(date) = NaiveDate::parse_from_str(value, DATE_FMT) {
        let naive = date.and_hms_opt(0, 0, 0)?;
        return Utc.from_local_datetime(&naive).single();
    }
    None
}

/// Extract a human display name from an `ATTENDEE`/`ORGANIZER` property.
///
/// Prefers the `CN` ("common name") parameter (surrounding quotes stripped);
/// otherwise strips a `mailto:` scheme from the value. Returns `None` for an
/// empty result so it is naturally filtered out of the attendees list.
pub(super) fn participant_display(prop: &icalendar::parser::Property<'_>) -> Option<String> {
    let cn = prop
        .params
        .iter()
        .find(|p| p.key.as_ref().eq_ignore_ascii_case("CN"))
        .and_then(|p| p.val.as_ref().map(|v| v.as_str().to_string()))
        .map(|s| s.trim_matches('"').to_string())
        .filter(|s| !s.is_empty());
    if let Some(name) = cn {
        return Some(name);
    }
    let val = prop.val.as_str();
    // RFC 3986: the scheme is case-insensitive (`MAILTO:` occurs in the wild).
    let name = val
        .get(..7)
        .filter(|p| p.eq_ignore_ascii_case("mailto:"))
        .map_or(val, |_| &val[7..])
        .trim();
    let name = name.trim_matches('"');
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}
