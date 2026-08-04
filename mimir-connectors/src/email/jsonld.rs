//! schema.org JSON-LD deterministic extraction (Phase 3, #249).
//!
//! Layer 2 of the [`super::EmailConnector::extract`] cascade: reads
//! machine-readable `schema.org` JSON-LD embedded in
//! `<script type="application/ld+json">` tags within the HTML body of
//! transactional emails and turns the high-signal types (`Order`,
//! `ParcelDelivery`, `FlightReservation`, `EventReservation`,
//! `LodgingReservation`, `Ticket`, and `ReservationPackage`) into
//! [`NormalizedFact`] clusters. No LLM — pure deterministic Rust parsing per
//! the project rule "logic in Rust, not prompts".
//!
//! # Cascade position
//!
//! Layer 1 is the iMIP calendar-invite extraction ([`super::EmailConnector`]
//! `extract_invites`, #200). This module is layer 2: it runs on the *same*
//! parsed message, scanning HTML parts for JSON-LD. Layer 3 is the C7 LLM
//! extraction (#201), which handles unstructured prose confirmations that
//! carry no machine-readable JSON-LD. By absorbing the deterministic
//! flight/booking/order facts here, C7's scope shrinks to genuinely
//! unstructured content.
//!
//! # Provenance
//!
//! Every fact carries `source_type = Connector` and the email's
//! `UIDVALIDITY`-qualified IMAP UID as `raw_reference` (matching the iMIP
//! layer and the persisted cursor format). The `connector_type` and
//! `extraction_method = StructuredParse` are set on the [`Provenance`] by the
//! supervisor's `run_cycle`, so this module only fills the per-fact fields.
//!
//! # Unrecognised types
//!
//! Any `@type` not in the recognised set is logged at `debug` level and
//! skipped — never guessed. This matches the acceptance criterion and the
//! connector error-handling philosophy ("malformed data → log and skip").
//!
//! [`Provenance`]: mimir_knowledge::normalize::Provenance

use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use mail_parser::MimeHeaders;
use mimir_knowledge::models::entity::EntityType;
use mimir_knowledge::models::enums::EventType;
use mimir_knowledge::normalize::NormalizedFact;
use serde_json::Value;
use tracing::debug;

use crate::ical::vevent_fact;
use mimir_knowledge::models::enums::RecurrenceType;

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Extract JSON-LD facts from an already-parsed [`mail_parser::Message`].
///
/// Separated from [`extract_facts`] so the cascade in `extract()` (which
/// already has a parsed `Message`) can reuse the same parser instance without
/// a second parse.
pub(crate) fn extract_facts_from_message(
    user_identity: Option<&str>,
    message: &mail_parser::Message<'_>,
    raw_ref: &str,
) -> Vec<NormalizedFact> {
    let mut facts = Vec::new();
    for part in &message.parts {
        if !part.is_content_type("text", "html") {
            continue;
        }
        let Some(html) = part.text_contents() else {
            debug!(
                raw_ref,
                "text/html part had no decodable text; skipping JSON-LD scan"
            );
            continue;
        };
        for block in extract_jsonld_blocks(html) {
            match serde_json::from_str::<Value>(block) {
                Ok(value) => {
                    for node in flatten_nodes(&value) {
                        facts.extend(extract_node_facts(user_identity, node, raw_ref));
                    }
                }
                Err(err) => {
                    debug!(raw_ref, error = %err, "skipping unparseable JSON-LD block");
                }
            }
        }
    }
    facts
}

// ---------------------------------------------------------------------------
// HTML <script type="application/ld+json"> extraction
// ---------------------------------------------------------------------------

/// Extract the text content of every `<script type="application/ld+json">`
/// block from an HTML string.
///
/// Returns slices into the original `html` (zero-copy). The scan is
/// case-insensitive for tag and attribute names (per the HTML spec). A
/// script element's text content terminates at the first `</script>` end tag
/// (HTML5 §12.1.2), so finding the next `</script>` is the correct
/// extraction — no HTML parser is needed for this narrow, well-defined task.
pub(crate) fn extract_jsonld_blocks(html: &str) -> Vec<&str> {
    // ASCII lowercasing is a 1:1 byte mapping, so byte offsets in the
    // lowercased string correspond exactly to offsets in the original.
    let lower = html.to_ascii_lowercase();
    let mut blocks = Vec::new();
    let mut pos = 0;
    let script_tag_len = "<script".len();
    let close_tag_len = "</script>".len();

    while let Some(rel) = lower[pos..].find("<script") {
        let tag_start = pos + rel;
        let after_tag_name = tag_start + script_tag_len;

        // Find the end of the opening `<script ...>` tag.
        let Some(greater_rel) = lower[after_tag_name..].find('>') else {
            break;
        };
        let tag_end = after_tag_name + greater_rel;
        let tag_inner = &html[after_tag_name..tag_end];

        // Content starts after `>`.
        let content_start = tag_end + 1;

        // Find the closing `</script>` (case-insensitive). Every `<script>`
        // element has a closing `</script>` — we must skip past it *regardless*
        // of whether this is a JSON-LD script, so JavaScript content
        // containing `<script` string literals (common in templating/tracking
        // snippets) is not re-scanned for JSON-LD blocks. HTML5 §12.1.2: a
        // script element's text content terminates at the first `</script>`
        // end tag.
        let Some(close_rel) = lower[content_start..].find("</script>") else {
            break;
        };
        let content_end = content_start + close_rel;

        if has_jsonld_type(tag_inner) {
            blocks.push(html[content_start..content_end].trim());
        }

        pos = content_end + close_tag_len;
    }
    blocks
}

/// Check whether a `<script>` tag's inner attribute string contains
/// `type="application/ld+json"` (case-insensitive).
fn has_jsonld_type(tag_inner: &str) -> bool {
    for (name, value) in parse_html_attributes(tag_inner) {
        if name.eq_ignore_ascii_case("type")
            && value.trim().eq_ignore_ascii_case("application/ld+json")
        {
            return true;
        }
    }
    false
}

/// Parse HTML attribute name=value pairs from the text between `<script` and
/// `>`.
///
/// Handles double-quoted, single-quoted, and unquoted values, and boolean
/// attributes (no `=`). Attribute names are matched case-insensitively by
/// callers. This is a minimal parser sufficient for `<script>` tag
/// attributes — it does not attempt to be a general HTML attribute parser.
fn parse_html_attributes(s: &str) -> Vec<(String, String)> {
    let chars: Vec<char> = s.chars().collect();
    let mut attrs = Vec::new();
    let mut i = 0;

    while i < chars.len() {
        // Skip whitespace.
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
        if i >= chars.len() {
            break;
        }

        // Read attribute name (up to `=`, whitespace, or end).
        let name_start = i;
        while i < chars.len() && !chars[i].is_whitespace() && chars[i] != '=' {
            i += 1;
        }
        let name: String = chars[name_start..i].iter().collect();
        if name.is_empty() {
            i += 1;
            continue;
        }

        // Skip whitespace before potential `=`.
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }

        if i < chars.len() && chars[i] == '=' {
            i += 1; // consume `=`
            while i < chars.len() && chars[i].is_whitespace() {
                i += 1;
            }
            if i >= chars.len() {
                attrs.push((name, String::new()));
                break;
            }
            let value = if chars[i] == '"' || chars[i] == '\'' {
                let quote = chars[i];
                i += 1; // skip opening quote
                let val_start = i;
                while i < chars.len() && chars[i] != quote {
                    i += 1;
                }
                let val: String = chars[val_start..i].iter().collect();
                if i < chars.len() {
                    i += 1; // skip closing quote
                }
                val
            } else {
                let val_start = i;
                while i < chars.len() && !chars[i].is_whitespace() {
                    i += 1;
                }
                chars[val_start..i].iter().collect()
            };
            attrs.push((name, value));
        } else {
            // Boolean attribute (no value).
            attrs.push((name, String::new()));
        }
    }
    attrs
}

// ---------------------------------------------------------------------------
// JSON-LD structural normalization
// ---------------------------------------------------------------------------

/// Flatten a parsed JSON-LD [`Value`] into a list of node objects (each a
/// `serde_json::Map`).
///
/// Handles the three structural forms JSON-LD can take:
/// - A single object with `@type` → one node.
/// - An array of objects → each object with `@type` is a node.
/// - A `@graph` wrapper (single object or array) → each `@graph` entry with
///   `@type` is a node.
///
/// Objects without `@type` (e.g. a bare `@context` wrapper) are skipped.
fn flatten_nodes(value: &Value) -> Vec<&serde_json::Map<String, Value>> {
    match value {
        Value::Object(map) => {
            if let Some(graph) = map.get("@graph") {
                return flatten_nodes(graph);
            }
            if map.contains_key("@type") {
                vec![map]
            } else {
                Vec::new()
            }
        }
        Value::Array(arr) => arr.iter().flat_map(flatten_nodes).collect(),
        _ => Vec::new(),
    }
}

/// Extract the `@type` values from a node (may be a string or array of
/// strings).
fn node_types(node: &serde_json::Map<String, Value>) -> Vec<String> {
    match node.get("@type") {
        Some(Value::String(s)) => vec![s.clone()],
        Some(Value::Array(arr)) => arr
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect(),
        _ => Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Schema.org type dispatch
// ---------------------------------------------------------------------------

/// Match a node's `@type` against the recognised set and extract facts.
///
/// If `@type` is an array, the first recognised type wins. Unrecognised
/// types are logged at `debug` level and produce no facts.
fn extract_node_facts(
    user: Option<&str>,
    node: &serde_json::Map<String, Value>,
    raw_ref: &str,
) -> Vec<NormalizedFact> {
    let types = node_types(node);
    for t in &types {
        match t.as_str() {
            "FlightReservation" => return flight_reservation_facts(user, node, raw_ref),
            "LodgingReservation" => return lodging_reservation_facts(user, node, raw_ref),
            "EventReservation" => return event_reservation_facts(user, node, raw_ref),
            "Order" => return order_facts(user, node, raw_ref),
            "ParcelDelivery" => return parcel_delivery_facts(user, node, raw_ref),
            "Ticket" => return ticket_facts(user, node, raw_ref),
            "ReservationPackage" => return reservation_package_facts(user, node, raw_ref),
            _ => {}
        }
    }
    if !types.is_empty() {
        debug!(
            raw_ref,
            types = ?types,
            "skipping unrecognised JSON-LD @type"
        );
    }
    Vec::new()
}

// ---------------------------------------------------------------------------
// Per-type extractors
// ---------------------------------------------------------------------------

/// Build a [`NormalizedFact`] with connector-level defaults (source type,
/// sensitivity, recurrence = None). Thin wrapper over [`vevent_fact`] to
/// avoid duplicating the `NormalizedFact` struct literal (DRY).
#[allow(clippy::too_many_arguments)] // constructor helper: every arg maps to a `NormalizedFact` field
fn jsonld_fact(
    subject: String,
    subject_type: EntityType,
    relationship_type: &str,
    object: String,
    object_type: Option<EntityType>,
    valid_from: Option<DateTime<Utc>>,
    valid_until: Option<DateTime<Utc>>,
    event_type: Option<EventType>,
    raw_ref: &str,
) -> NormalizedFact {
    vevent_fact(
        subject,
        subject_type,
        relationship_type,
        object,
        object_type,
        valid_from,
        valid_until,
        RecurrenceType::None,
        event_type,
        raw_ref,
    )
}

/// `FlightReservation` → flight fact cluster.
///
/// Emits:
/// 1. `user has_flight <flight>` (Event, `Appointment`, temporal =
///    departure → arrival) — only when `user_identity` is set **and** a
///    parseable `departureTime` is present (an `Appointment` overlay needs a
///    `valid_from`, matching the iMIP layer's `DTSTART` requirement).
/// 2. `<flight> departs_from <airport>` (Place) — always.
/// 3. `<flight> arrives_at <airport>` (Place) — always.
/// 4. `<flight> operated_by <airline>` (Organization) — always.
fn flight_reservation_facts(
    user: Option<&str>,
    node: &serde_json::Map<String, Value>,
    raw_ref: &str,
) -> Vec<NormalizedFact> {
    let Some(flight) = node.get("reservationFor").and_then(|v| v.as_object()) else {
        debug!(
            raw_ref,
            "FlightReservation without reservationFor; skipping"
        );
        return Vec::new();
    };
    let flight_name = match flight_name(flight) {
        Some(name) => name,
        None => {
            debug!(
                raw_ref,
                "FlightReservation could not derive a flight name; skipping"
            );
            return Vec::new();
        }
    };
    let departure = flight.get("departureTime").and_then(parse_datetime);
    let arrival = flight.get("arrivalTime").and_then(parse_datetime);

    let mut facts = Vec::new();

    if let (Some(user), Some(departure)) = (user, departure) {
        facts.push(jsonld_fact(
            user.to_string(),
            EntityType::Person,
            "has_flight",
            flight_name.clone(),
            Some(EntityType::Event),
            Some(departure),
            arrival,
            Some(EventType::Appointment),
            raw_ref,
        ));
    }

    if let Some(origin) = airport_name(flight.get("departureAirport")) {
        facts.push(jsonld_fact(
            flight_name.clone(),
            EntityType::Event,
            "departs_from",
            origin,
            Some(EntityType::Place),
            None,
            None,
            None,
            raw_ref,
        ));
    }
    if let Some(dest) = airport_name(flight.get("arrivalAirport")) {
        facts.push(jsonld_fact(
            flight_name.clone(),
            EntityType::Event,
            "arrives_at",
            dest,
            Some(EntityType::Place),
            None,
            None,
            None,
            raw_ref,
        ));
    }
    if let Some(airline) = string_or_name(flight.get("airline")) {
        facts.push(jsonld_fact(
            flight_name,
            EntityType::Event,
            "operated_by",
            airline,
            Some(EntityType::Organization),
            None,
            None,
            None,
            raw_ref,
        ));
    }
    facts
}

/// `LodgingReservation` → hotel booking fact cluster.
///
/// Emits:
/// 1. `user has_booking <hotel>` (Event, `Appointment`, temporal = checkin →
///    checkout) — only when `user_identity` is set **and** a parseable
///    `checkinDate` is present (an `Appointment` overlay needs a `valid_from`).
/// 2. `<booking> located_in <place>` (Place) — always, when a location
///    distinct from the booking name is available.
fn lodging_reservation_facts(
    user: Option<&str>,
    node: &serde_json::Map<String, Value>,
    raw_ref: &str,
) -> Vec<NormalizedFact> {
    let lodging = node.get("reservationFor").and_then(|v| v.as_object());
    let hotel_name = lodging
        .and_then(|l| string_or_name_field(l, "name"))
        .or_else(|| string_or_name(node.get("lodgingUnitDescription")));
    let Some(hotel_name) = hotel_name else {
        debug!(raw_ref, "LodgingReservation without a hotel name; skipping");
        return Vec::new();
    };
    let checkin = node.get("checkinDate").and_then(parse_datetime);
    let checkout = node.get("checkoutDate").and_then(parse_datetime);

    let mut facts = Vec::new();

    if let (Some(user), Some(checkin)) = (user, checkin) {
        facts.push(jsonld_fact(
            user.to_string(),
            EntityType::Person,
            "has_booking",
            hotel_name.clone(),
            Some(EntityType::Event),
            Some(checkin),
            checkout,
            Some(EventType::Appointment),
            raw_ref,
        ));
    }

    // Location: prefer the lodging business address, fall back to the name.
    // A location identical to the booking name carries no information, so it
    // is skipped rather than emitted as a self-referential `located_in` fact.
    let loc_name = lodging
        .and_then(|l| l.get("address"))
        .and_then(|a| string_or_name_field(a.as_object()?, "streetAddress"))
        .or_else(|| lodging.and_then(|l| string_or_name_field(l, "name")))
        .filter(|loc| loc != &hotel_name);

    if let Some(loc_name) = loc_name {
        facts.push(jsonld_fact(
            hotel_name,
            EntityType::Event,
            "located_in",
            loc_name,
            Some(EntityType::Place),
            None,
            None,
            None,
            raw_ref,
        ));
    }
    facts
}

/// `EventReservation` → event fact cluster.
///
/// Emits:
/// 1. `user has_event <event>` (Event, `Appointment`, temporal = start → end)
///    — only when `user_identity` is set **and** a parseable `startDate` is
///    present (an `Appointment` overlay needs a `valid_from`).
/// 2. `<event> located_in <venue>` (Place) — always, when a venue is present.
fn event_reservation_facts(
    user: Option<&str>,
    node: &serde_json::Map<String, Value>,
    raw_ref: &str,
) -> Vec<NormalizedFact> {
    let Some(event) = node.get("reservationFor").and_then(|v| v.as_object()) else {
        debug!(raw_ref, "EventReservation without reservationFor; skipping");
        return Vec::new();
    };
    let Some(event_name) = string_or_name_field(event, "name") else {
        debug!(raw_ref, "EventReservation without event name; skipping");
        return Vec::new();
    };
    let start = event.get("startDate").and_then(parse_datetime);
    let end = event.get("endDate").and_then(parse_datetime);

    let mut facts = Vec::new();

    if let (Some(user), Some(start)) = (user, start) {
        facts.push(jsonld_fact(
            user.to_string(),
            EntityType::Person,
            "has_event",
            event_name.clone(),
            Some(EntityType::Event),
            Some(start),
            end,
            Some(EventType::Appointment),
            raw_ref,
        ));
    }

    if let Some(venue) = string_or_name(event.get("location")) {
        facts.push(jsonld_fact(
            event_name,
            EntityType::Event,
            "located_in",
            venue,
            Some(EntityType::Place),
            None,
            None,
            None,
            raw_ref,
        ));
    }
    facts
}

/// `Order` → purchase fact cluster.
///
/// Emits:
/// 1. `user has_order <order>` (Event, `event_type = None`, temporal =
///    orderDate) — only when `user_identity` is set. An order is a fact, not
///    a scheduled event; the delivery (if any) is tracked by
///    `ParcelDelivery`.
/// 2. `<order> purchased_from <merchant>` (Organization) — always, when a
///    merchant is present.
fn order_facts(
    user: Option<&str>,
    node: &serde_json::Map<String, Value>,
    raw_ref: &str,
) -> Vec<NormalizedFact> {
    let order_name = string_or_name_field(node, "orderNumber")
        .or_else(|| string_or_name_field(node, "description"));
    let Some(order_name) = order_name else {
        debug!(
            raw_ref,
            "Order without an orderNumber or description; skipping"
        );
        return Vec::new();
    };
    let order_date = node.get("orderDate").and_then(parse_datetime);

    let mut facts = Vec::new();

    if let Some(user) = user {
        facts.push(jsonld_fact(
            user.to_string(),
            EntityType::Person,
            "has_order",
            order_name.clone(),
            Some(EntityType::Event),
            order_date,
            None,
            None,
            raw_ref,
        ));
    }

    if let Some(merchant) = string_or_name(node.get("merchant")) {
        facts.push(jsonld_fact(
            order_name,
            EntityType::Event,
            "purchased_from",
            merchant,
            Some(EntityType::Organization),
            None,
            None,
            None,
            raw_ref,
        ));
    }
    facts
}

/// `ParcelDelivery` → delivery fact cluster.
///
/// Emits:
/// 1. `user has_delivery <tracking>` (Event, `Reminder`, temporal =
///    expectedArrival window) — only when `user_identity` is set. A delivery
///    is something to track, not an event the user attends.
/// 2. `<delivery> shipped_by <carrier>` (Organization) — always.
/// 3. `<delivery> delivered_to <address>` (Place) — always, when an address
///    is present.
fn parcel_delivery_facts(
    user: Option<&str>,
    node: &serde_json::Map<String, Value>,
    raw_ref: &str,
) -> Vec<NormalizedFact> {
    let Some(tracking) = string_or_name_field(node, "trackingNumber") else {
        debug!(raw_ref, "ParcelDelivery without trackingNumber; skipping");
        return Vec::new();
    };
    let expected_from = node.get("expectedArrivalFrom").and_then(parse_datetime);
    let expected_until = node.get("expectedArrivalUntil").and_then(parse_datetime);

    let mut facts = Vec::new();

    if let Some(user) = user {
        facts.push(jsonld_fact(
            user.to_string(),
            EntityType::Person,
            "has_delivery",
            tracking.clone(),
            Some(EntityType::Event),
            expected_from,
            expected_until,
            Some(EventType::Reminder),
            raw_ref,
        ));
    }

    if let Some(carrier) = string_or_name(node.get("carrier")) {
        facts.push(jsonld_fact(
            tracking.clone(),
            EntityType::Event,
            "shipped_by",
            carrier,
            Some(EntityType::Organization),
            None,
            None,
            None,
            raw_ref,
        ));
    }

    if let Some(addr) = node.get("deliveryAddress").and_then(|v| v.as_object()) {
        if let Some(street) = string_or_name_field(addr, "streetAddress") {
            facts.push(jsonld_fact(
                tracking,
                EntityType::Event,
                "delivered_to",
                street,
                Some(EntityType::Place),
                None,
                None,
                None,
                raw_ref,
            ));
        }
    }
    facts
}

/// `Ticket` → ticket fact cluster.
///
/// Emits:
/// 1. `user has_ticket <ticket>` (Event, `event_type = None`) — only when
///    `user_identity` is set. A `Ticket` is too generic to assume it is a
///    time-bound event (it could be for a flight, a concert, or a raffle);
///    the `event_type` is left `None` so the events subsystem does not
///    create an overlay unless other signals (temporal bounds, requires
///    action) qualify it.
/// 2. `<ticket> issued_by <issuer>` (Organization) — always, when an issuer
///    is present.
fn ticket_facts(
    user: Option<&str>,
    node: &serde_json::Map<String, Value>,
    raw_ref: &str,
) -> Vec<NormalizedFact> {
    let ticket_name = string_or_name_field(node, "ticketNumber")
        .or_else(|| string_or_name_field(node, "description"));
    let Some(ticket_name) = ticket_name else {
        debug!(
            raw_ref,
            "Ticket without a ticketNumber or description; skipping"
        );
        return Vec::new();
    };
    let issued = node.get("dateIssued").and_then(parse_datetime);

    let mut facts = Vec::new();

    if let Some(user) = user {
        facts.push(jsonld_fact(
            user.to_string(),
            EntityType::Person,
            "has_ticket",
            ticket_name.clone(),
            Some(EntityType::Event),
            issued,
            None,
            None,
            raw_ref,
        ));
    }

    if let Some(issuer) = string_or_name(node.get("issuedBy")) {
        facts.push(jsonld_fact(
            ticket_name,
            EntityType::Event,
            "issued_by",
            issuer,
            Some(EntityType::Organization),
            None,
            None,
            None,
            raw_ref,
        ));
    }
    facts
}

/// `ReservationPackage` → flatten `subReservation` and process each.
///
/// Airlines bundle multi-leg flights as a `ReservationPackage` with a
/// `subReservation` array. Each sub-reservation is dispatched by its own
/// `@type`, producing independent fact clusters that share the same
/// `raw_reference`.
fn reservation_package_facts(
    user: Option<&str>,
    node: &serde_json::Map<String, Value>,
    raw_ref: &str,
) -> Vec<NormalizedFact> {
    let mut facts = Vec::new();
    if let Some(sub) = node.get("subReservation") {
        for sub_node in flatten_nodes(sub) {
            facts.extend(extract_node_facts(user, sub_node, raw_ref));
        }
    }
    if facts.is_empty() {
        debug!(
            raw_ref,
            "ReservationPackage with no processable subReservation; skipping"
        );
    }
    facts
}

// ---------------------------------------------------------------------------
// JSON-LD field helpers
// ---------------------------------------------------------------------------

/// Coerce a JSON scalar (`String` or `Number`) to a trimmed, non-empty
/// string. schema.org types many identifier fields (`orderNumber`,
/// `trackingNumber`, `ticketNumber`, `flightNumber`, `iataCode`) as `Text`,
/// but producers frequently emit them as JSON numbers, so numbers are
/// stringified rather than dropped.
fn scalar_string(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => {
            let trimmed = s.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        }
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// Extract a human-readable string from a value that may be a plain string
/// or an object with a `name` field (the common `string-or-object` JSON-LD
/// pattern).
fn string_or_name(value: Option<&Value>) -> Option<String> {
    let value = value?;
    match value {
        Value::String(_) => scalar_string(value),
        Value::Object(map) => string_or_name_field(map, "name"),
        _ => None,
    }
}

/// Read a `name` (or alternate) field from a JSON object as a non-empty
/// trimmed string.
fn string_or_name_field(map: &serde_json::Map<String, Value>, field: &str) -> Option<String> {
    let val = map.get(field)?;
    match val {
        Value::Array(arr) => {
            // Some producers wrap a single value in an array.
            arr.iter().filter_map(scalar_string).next()
        }
        // Strings and numbers are coerced by `scalar_string`; objects,
        // booleans, and null are not name-like and yield `None`.
        _ => scalar_string(val),
    }
}

/// Extract an airport display name, preferring the `name` field and falling
/// back to `iataCode`.
fn airport_name(value: Option<&Value>) -> Option<String> {
    let value = value?;
    // Object: prefer `name`, fall back to `iataCode`.
    if let Some(obj) = value.as_object() {
        if let Some(name) = string_or_name_field(obj, "name") {
            return Some(name);
        }
        return obj.get("iataCode").and_then(scalar_string);
    }
    // Bare string (e.g. "LHR").
    value
        .as_str()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}
///
/// Prefers `"{airline} {flightNumber}"` (the standard way people refer to
/// flights); falls back to `"{origin} → {destination}"` using airport names
/// or IATA codes.
fn flight_name(flight: &serde_json::Map<String, Value>) -> Option<String> {
    let airline = string_or_name(flight.get("airline"));
    let flight_num = flight.get("flightNumber").and_then(scalar_string);

    if let (Some(a), Some(n)) = (&airline, &flight_num) {
        return Some(format!("{a} {n}"));
    }
    let origin = airport_name(flight.get("departureAirport"));
    let dest = airport_name(flight.get("arrivalAirport"));
    match (origin, dest) {
        (Some(o), Some(d)) => Some(format!("{o} → {d}")),
        _ => None,
    }
}

/// Parse an ISO 8601 / RFC 3339 datetime or date-only string into UTC.
///
/// Handles full RFC 3339 (with timezone offset), naive datetime (treated as
/// UTC), naive datetime (treated as UTC, with or without fractional seconds
/// and with or without a seconds field), and date-only (`YYYY-MM-DD` →
/// midnight UTC). Returns `None` for unparseable values so a bad date never
/// drops the entire fact cluster.
fn parse_datetime(value: &Value) -> Option<DateTime<Utc>> {
    let s = value.as_str()?;
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Some(dt.with_timezone(&Utc));
    }
    if let Ok(dt) = NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S") {
        return Some(dt.and_utc());
    }
    if let Ok(dt) = NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f") {
        return Some(dt.and_utc());
    }
    if let Ok(dt) = NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M") {
        return Some(dt.and_utc());
    }
    if let Ok(date) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return Some(date.and_hms_opt(0, 0, 0)?.and_utc());
    }
    None
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use mimir_knowledge::models::source::SourceType;

    // --- HTML <script> extraction ----------------------------------------

    #[test]
    fn extract_jsonld_blocks_standard() {
        let html = r#"<html><body>
<script type="application/ld+json">{"@type":"Order","orderNumber":"X"}</script>
</body></html>"#;
        let blocks = extract_jsonld_blocks(html);
        assert_eq!(blocks.len(), 1);
        assert!(blocks[0].contains("\"@type\":\"Order\""));
    }

    #[test]
    fn extract_jsonld_blocks_single_quotes() {
        let html = r#"<script type='application/ld+json'>{"@type":"Order"}</script>"#;
        let blocks = extract_jsonld_blocks(html);
        assert_eq!(blocks.len(), 1);
    }

    #[test]
    fn extract_jsonld_blocks_type_not_first_attribute() {
        let html = r#"<script data-x="y" type="application/ld+json">{"@type":"Order"}</script>"#;
        let blocks = extract_jsonld_blocks(html);
        assert_eq!(blocks.len(), 1);
    }

    #[test]
    fn extract_jsonld_blocks_multiple() {
        let html = r#"<script type="application/ld+json">{"@type":"Order"}</script>
<script type="application/ld+json">{"@type":"ParcelDelivery"}</script>"#;
        let blocks = extract_jsonld_blocks(html);
        assert_eq!(blocks.len(), 2);
    }

    #[test]
    fn extract_jsonld_blocks_skips_javascript() {
        let html = r#"<script type="text/javascript">console.log("hi")</script>
<script type="application/ld+json">{"@type":"Order"}</script>"#;
        let blocks = extract_jsonld_blocks(html);
        assert_eq!(blocks.len(), 1);
    }

    #[test]
    fn extract_jsonld_blocks_case_insensitive() {
        let html = r#"<SCRIPT TYPE="application/ld+json">{"@type":"Order"}</SCRIPT>"#;
        let blocks = extract_jsonld_blocks(html);
        assert_eq!(blocks.len(), 1);
    }

    #[test]
    fn extract_jsonld_blocks_trims_whitespace_in_type_attribute() {
        // HTML5 strips ASCII whitespace from the `type` attribute value before
        // comparing it, so padded values must still match.
        let html = r#"<script type=" application/ld+json ">{"@type":"Order"}</script>"#;
        let blocks = extract_jsonld_blocks(html);
        assert_eq!(blocks.len(), 1);
    }

    #[test]
    fn extract_jsonld_blocks_no_blocks() {
        let html = "<html><body><p>hello</p></body></html>";
        assert!(extract_jsonld_blocks(html).is_empty());
    }

    #[test]
    fn extract_jsonld_blocks_no_data_type_attribute() {
        // `data-type` is not `type` — must not match.
        let html = r#"<script data-type="application/ld+json">{"@type":"Order"}</script>"#;
        assert!(extract_jsonld_blocks(html).is_empty());
    }

    #[test]
    fn extract_jsonld_blocks_skips_js_with_embedded_jsonld_string() {
        // A JavaScript script whose body contains a string literal that looks
        // like a JSON-LD block must NOT produce a false positive. The scanner
        // must skip past the JS script's closing </script> before resuming the
        // search, so the embedded `<script type="application/ld+json">` inside
        // the JS string is never seen as a real tag.
        let html = r#"<script type="text/javascript">
  var x = '<script type="application/ld+json">{"@type":"Order","orderNumber":"FAKE"}</script>';
</script>
<script type="application/ld+json">{"@type":"Order","orderNumber":"REAL"}</script>"#;
        let blocks = extract_jsonld_blocks(html);
        assert_eq!(
            blocks.len(),
            1,
            "only the real JSON-LD block should be extracted"
        );
        assert!(
            blocks[0].contains("REAL"),
            "must extract the real block, not the JS string"
        );
    }

    #[test]
    fn extract_jsonld_blocks_trims_whitespace() {
        let html = r#"<script type="application/ld+json">  {"@type":"Order"}  </script>"#;
        let blocks = extract_jsonld_blocks(html);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0], r#"{"@type":"Order"}"#);
    }

    #[test]
    fn parse_html_attributes_basic() {
        let attrs = parse_html_attributes(r#"type="application/ld+json" data-x="y""#);
        assert_eq!(attrs.len(), 2);
        assert_eq!(attrs[0].0, "type");
        assert_eq!(attrs[0].1, "application/ld+json");
        assert_eq!(attrs[1].0, "data-x");
    }

    #[test]
    fn parse_html_attributes_single_quotes_and_unquoted() {
        let attrs = parse_html_attributes(r#"type='application/ld+json' defer"#);
        assert_eq!(attrs.len(), 2);
        assert_eq!(attrs[0].1, "application/ld+json");
        assert_eq!(attrs[1].0, "defer");
        assert!(attrs[1].1.is_empty());
    }

    // --- JSON-LD structural normalization --------------------------------

    #[test]
    fn flatten_single_object_with_type() {
        let v: Value = serde_json::from_str(r#"{"@type":"Order","orderNumber":"X"}"#).unwrap();
        let nodes = flatten_nodes(&v);
        assert_eq!(nodes.len(), 1);
        assert_eq!(node_types(nodes[0]), vec!["Order"]);
    }

    #[test]
    fn flatten_array_of_objects() {
        let v: Value =
            serde_json::from_str(r#"[{"@type":"Order"},{"@type":"ParcelDelivery"}]"#).unwrap();
        let nodes = flatten_nodes(&v);
        assert_eq!(nodes.len(), 2);
    }

    #[test]
    fn flatten_graph_wrapper_array() {
        let v: Value = serde_json::from_str(
            r#"{"@context":"https://schema.org","@graph":[{"@type":"Order"},{"@type":"ParcelDelivery"}]}"#,
        )
        .unwrap();
        let nodes = flatten_nodes(&v);
        assert_eq!(nodes.len(), 2);
    }

    #[test]
    fn flatten_graph_single_object() {
        let v: Value =
            serde_json::from_str(r#"{"@graph":{"@type":"Order","orderNumber":"X"}}"#).unwrap();
        let nodes = flatten_nodes(&v);
        assert_eq!(nodes.len(), 1);
    }

    #[test]
    fn flatten_context_wrapper_without_type_skipped() {
        let v: Value = serde_json::from_str(r#"{"@context":"https://schema.org"}"#).unwrap();
        assert!(flatten_nodes(&v).is_empty());
    }

    #[test]
    fn flatten_non_object_skipped() {
        let v: Value = serde_json::from_str(r#""just a string""#).unwrap();
        assert!(flatten_nodes(&v).is_empty());
    }

    #[test]
    fn node_types_array() {
        let v: Value =
            serde_json::from_str(r#"{"@type":["FlightReservation","Reservation"]}"#).unwrap();
        let nodes = flatten_nodes(&v);
        assert_eq!(
            node_types(nodes[0]),
            vec!["FlightReservation", "Reservation"]
        );
    }

    // --- Unrecognised types are skipped ----------------------------------

    #[test]
    fn unrecognised_type_produces_no_facts() {
        let v: Value = serde_json::from_str(r#"{"@type":"Person","name":"Devansh"}"#).unwrap();
        let nodes = flatten_nodes(&v);
        let facts = extract_node_facts(Some("Devansh"), nodes[0], "42");
        assert!(facts.is_empty());
    }

    // --- FlightReservation -----------------------------------------------

    fn flight_reservation_json() -> &'static str {
        r#"{"@context":"https://schema.org","@type":"FlightReservation","reservationId":"ABC123","passengerName":"Devansh Bhavsar","reservationFor":{"@type":"Flight","flightNumber":"123","airline":{"@type":"Airline","name":"British Airways"},"departureAirport":{"@type":"Airport","name":"Heathrow Airport","iataCode":"LHR"},"departureTime":"2025-08-15T10:00:00+01:00","arrivalAirport":{"@type":"Airport","name":"Fiumicino Airport","iataCode":"FCO"},"arrivalTime":"2025-08-15T13:30:00+02:00"}}"#
    }

    #[test]
    fn flight_reservation_facts_with_identity() {
        let v: Value = serde_json::from_str(flight_reservation_json()).unwrap();
        let nodes = flatten_nodes(&v);
        let facts = extract_node_facts(Some("Devansh"), nodes[0], "17:42");
        // 1 primary + departs_from + arrives_at + operated_by = 4
        assert_eq!(facts.len(), 4);

        let primary = &facts[0];
        assert_eq!(primary.subject, "Devansh");
        assert_eq!(primary.subject_type, EntityType::Person);
        assert_eq!(primary.relationship_type, "has_flight");
        assert_eq!(primary.object, "British Airways 123");
        assert_eq!(primary.object_type, Some(EntityType::Event));
        assert!(primary.valid_from.is_some());
        assert!(primary.valid_until.is_some());
        assert_eq!(primary.event_type, Some(EventType::Appointment));
        assert_eq!(primary.raw_reference.as_deref(), Some("17:42"));
        assert_eq!(primary.source_type, SourceType::Connector);

        let departs = &facts[1];
        assert_eq!(departs.relationship_type, "departs_from");
        assert_eq!(departs.object, "Heathrow Airport");
        assert_eq!(departs.object_type, Some(EntityType::Place));
        assert!(departs.valid_from.is_none());
        assert!(departs.event_type.is_none());

        let arrives = &facts[2];
        assert_eq!(arrives.relationship_type, "arrives_at");
        assert_eq!(arrives.object, "Fiumicino Airport");

        let airline = &facts[3];
        assert_eq!(airline.relationship_type, "operated_by");
        assert_eq!(airline.object, "British Airways");
        assert_eq!(airline.object_type, Some(EntityType::Organization));
    }

    #[test]
    fn flight_reservation_facts_without_identity_still_emits_secondary() {
        let v: Value = serde_json::from_str(flight_reservation_json()).unwrap();
        let nodes = flatten_nodes(&v);
        let facts = extract_node_facts(None, nodes[0], "17:42");
        // No primary has_flight, but departs_from + arrives_at + operated_by
        assert_eq!(facts.len(), 3);
        assert!(facts.iter().all(|f| f.relationship_type != "has_flight"));
    }

    #[test]
    fn flight_reservation_falls_back_to_iata_for_airport_name() {
        let json = r#"{"@type":"FlightReservation","reservationFor":{"@type":"Flight","flightNumber":"456","airline":"EasyJet","departureAirport":{"iataCode":"LTN"},"arrivalAirport":{"iataCode":"FCO"},"departureTime":"2025-09-01T06:00:00Z","arrivalTime":"2025-09-01T09:00:00Z"}}"#;
        let v: Value = serde_json::from_str(json).unwrap();
        let nodes = flatten_nodes(&v);
        let facts = extract_node_facts(Some("Devansh"), nodes[0], "1:1");
        let departs = facts
            .iter()
            .find(|f| f.relationship_type == "departs_from")
            .unwrap();
        assert_eq!(departs.object, "LTN");
    }

    #[test]
    fn flight_name_falls_back_to_route_when_no_airline_or_number() {
        let json = r#"{"@type":"FlightReservation","reservationFor":{"@type":"Flight","departureAirport":{"name":"London"},"arrivalAirport":{"name":"Rome"},"departureTime":"2025-09-01T06:00:00Z","arrivalTime":"2025-09-01T09:00:00Z"}}"#;
        let v: Value = serde_json::from_str(json).unwrap();
        let nodes = flatten_nodes(&v);
        let facts = extract_node_facts(Some("Devansh"), nodes[0], "1:1");
        let primary = &facts[0];
        assert_eq!(primary.object, "London → Rome");
    }

    #[test]
    fn flight_reservation_without_reservation_for_skipped() {
        let json = r#"{"@type":"FlightReservation","reservationId":"X"}"#;
        let v: Value = serde_json::from_str(json).unwrap();
        let nodes = flatten_nodes(&v);
        assert!(extract_node_facts(Some("Devansh"), nodes[0], "1:1").is_empty());
    }

    #[test]
    fn flight_reservation_without_departure_time_skips_primary() {
        // An `Appointment` needs a `valid_from`; with no `departureTime` the
        // primary `has_flight` fact is skipped, but secondary facts still fire.
        let json = r#"{"@type":"FlightReservation","reservationFor":{"@type":"Flight","flightNumber":"123","airline":{"@type":"Airline","name":"BA"},"departureAirport":{"name":"LHR"},"arrivalAirport":{"name":"JFK"},"arrivalTime":"2025-08-15T14:00:00Z"}}"#;
        let v: Value = serde_json::from_str(json).unwrap();
        let nodes = flatten_nodes(&v);
        let facts = extract_node_facts(Some("Devansh"), nodes[0], "1:1");
        assert!(facts.iter().all(|f| f.relationship_type != "has_flight"));
        // departs_from + arrives_at + operated_by
        assert_eq!(facts.len(), 3);
    }

    // --- LodgingReservation ----------------------------------------------

    #[test]
    fn lodging_reservation_facts_with_identity() {
        let json = r#"{"@type":"LodgingReservation","reservationId":"H1","checkinDate":"2025-08-20","checkoutDate":"2025-08-25","reservationFor":{"@type":"LodgingBusiness","name":"Grand Hotel","address":{"@type":"PostalAddress","streetAddress":"123 Via Roma"}}}"#;
        let v: Value = serde_json::from_str(json).unwrap();
        let nodes = flatten_nodes(&v);
        let facts = extract_node_facts(Some("Devansh"), nodes[0], "17:42");
        // 1 primary + 1 location = 2
        assert_eq!(facts.len(), 2);

        let primary = &facts[0];
        assert_eq!(primary.relationship_type, "has_booking");
        assert_eq!(primary.object, "Grand Hotel");
        assert_eq!(primary.event_type, Some(EventType::Appointment));
        assert!(primary.valid_from.is_some());
        assert!(primary.valid_until.is_some());

        let loc = &facts[1];
        assert_eq!(loc.relationship_type, "located_in");
        assert_eq!(loc.object, "123 Via Roma");
        assert_eq!(loc.object_type, Some(EntityType::Place));
    }

    #[test]
    fn lodging_reservation_without_identity_emits_location() {
        // A distinct address is emitted as a secondary fact even without a
        // canonical user identity.
        let json = r#"{"@type":"LodgingReservation","checkinDate":"2025-08-20","checkoutDate":"2025-08-25","reservationFor":{"@type":"LodgingBusiness","name":"Grand Hotel","address":{"@type":"PostalAddress","streetAddress":"123 Via Roma"}}}"#;
        let v: Value = serde_json::from_str(json).unwrap();
        let nodes = flatten_nodes(&v);
        let facts = extract_node_facts(None, nodes[0], "1:1");
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].relationship_type, "located_in");
        assert_eq!(facts[0].object, "123 Via Roma");
    }

    #[test]
    fn lodging_reservation_without_address_skips_self_referential_location() {
        // No structured address: falling back to the booking name would emit
        // `Grand Hotel located_in Grand Hotel`, so the fact is skipped.
        let json = r#"{"@type":"LodgingReservation","checkinDate":"2025-08-20","checkoutDate":"2025-08-25","reservationFor":{"@type":"LodgingBusiness","name":"Grand Hotel"}}"#;
        let v: Value = serde_json::from_str(json).unwrap();
        let nodes = flatten_nodes(&v);
        let facts = extract_node_facts(None, nodes[0], "1:1");
        assert!(facts.is_empty());
    }

    #[test]
    fn lodging_reservation_without_checkin_skips_primary() {
        // No `checkinDate` → no `valid_from` → the primary `has_booking`
        // `Appointment` fact is skipped. A distinct address still emits a
        // secondary `located_in` fact.
        let json = r#"{"@type":"LodgingReservation","checkoutDate":"2025-08-25","reservationFor":{"@type":"LodgingBusiness","name":"Grand Hotel","address":{"@type":"PostalAddress","streetAddress":"123 Via Roma"}}}"#;
        let v: Value = serde_json::from_str(json).unwrap();
        let nodes = flatten_nodes(&v);
        let facts = extract_node_facts(Some("Devansh"), nodes[0], "1:1");
        assert!(facts.iter().all(|f| f.relationship_type != "has_booking"));
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].relationship_type, "located_in");
    }

    // --- EventReservation -------------------------------------------------

    #[test]
    fn event_reservation_facts_with_identity() {
        let json = r#"{"@type":"EventReservation","reservationId":"E1","reservationFor":{"@type":"Event","name":"Symphony Concert","startDate":"2025-09-10T19:30:00+02:00","endDate":"2025-09-10T21:00:00+02:00","location":{"@type":"Place","name":"Royal Albert Hall"}}}"#;
        let v: Value = serde_json::from_str(json).unwrap();
        let nodes = flatten_nodes(&v);
        let facts = extract_node_facts(Some("Devansh"), nodes[0], "17:42");
        // 1 primary + 1 location = 2
        assert_eq!(facts.len(), 2);

        let primary = &facts[0];
        assert_eq!(primary.relationship_type, "has_event");
        assert_eq!(primary.object, "Symphony Concert");
        assert_eq!(primary.event_type, Some(EventType::Appointment));

        let loc = &facts[1];
        assert_eq!(loc.relationship_type, "located_in");
        assert_eq!(loc.object, "Royal Albert Hall");
    }

    #[test]
    fn event_reservation_without_start_date_skips_primary() {
        // No `startDate` → no `valid_from` → the primary `has_event`
        // `Appointment` fact is skipped; the venue `located_in` still fires.
        let json = r#"{"@type":"EventReservation","reservationFor":{"@type":"Event","name":"Symphony Concert","endDate":"2025-09-10T21:00:00+02:00","location":{"@type":"Place","name":"Royal Albert Hall"}}}"#;
        let v: Value = serde_json::from_str(json).unwrap();
        let nodes = flatten_nodes(&v);
        let facts = extract_node_facts(Some("Devansh"), nodes[0], "1:1");
        assert!(facts.iter().all(|f| f.relationship_type != "has_event"));
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].relationship_type, "located_in");
    }

    // --- Order ------------------------------------------------------------

    #[test]
    fn order_facts_with_identity() {
        let json = r#"{"@type":"Order","orderNumber":"ORD-99","orderDate":"2025-08-01","merchant":{"@type":"Organization","name":"Acme Corp"}}"#;
        let v: Value = serde_json::from_str(json).unwrap();
        let nodes = flatten_nodes(&v);
        let facts = extract_node_facts(Some("Devansh"), nodes[0], "17:42");
        // 1 primary + 1 merchant = 2
        assert_eq!(facts.len(), 2);

        let primary = &facts[0];
        assert_eq!(primary.relationship_type, "has_order");
        assert_eq!(primary.object, "ORD-99");
        assert_eq!(primary.event_type, None);
        assert!(primary.valid_from.is_some());

        let merchant = &facts[1];
        assert_eq!(merchant.relationship_type, "purchased_from");
        assert_eq!(merchant.object, "Acme Corp");
        assert_eq!(merchant.object_type, Some(EntityType::Organization));
    }

    #[test]
    fn order_without_merchant_emits_only_primary() {
        let json = r#"{"@type":"Order","orderNumber":"ORD-1"}"#;
        let v: Value = serde_json::from_str(json).unwrap();
        let nodes = flatten_nodes(&v);
        let facts = extract_node_facts(Some("Devansh"), nodes[0], "1:1");
        assert_eq!(facts.len(), 1);
    }

    // --- ParcelDelivery ---------------------------------------------------

    #[test]
    fn parcel_delivery_facts_with_identity() {
        let json = r#"{"@type":"ParcelDelivery","trackingNumber":"TRK123","expectedArrivalFrom":"2025-08-05","expectedArrivalUntil":"2025-08-07","carrier":{"@type":"Organization","name":"DHL"},"deliveryAddress":{"@type":"PostalAddress","streetAddress":"10 Downing St"}}"#;
        let v: Value = serde_json::from_str(json).unwrap();
        let nodes = flatten_nodes(&v);
        let facts = extract_node_facts(Some("Devansh"), nodes[0], "17:42");
        // 1 primary + 1 carrier + 1 address = 3
        assert_eq!(facts.len(), 3);

        let primary = &facts[0];
        assert_eq!(primary.relationship_type, "has_delivery");
        assert_eq!(primary.object, "TRK123");
        assert_eq!(primary.event_type, Some(EventType::Reminder));
        assert!(primary.valid_from.is_some());
        assert!(primary.valid_until.is_some());

        let carrier = &facts[1];
        assert_eq!(carrier.relationship_type, "shipped_by");
        assert_eq!(carrier.object, "DHL");

        let addr = &facts[2];
        assert_eq!(addr.relationship_type, "delivered_to");
        assert_eq!(addr.object, "10 Downing St");
    }

    // --- Ticket -----------------------------------------------------------

    #[test]
    fn ticket_facts_with_identity() {
        let json = r#"{"@type":"Ticket","ticketNumber":"TKT-7","dateIssued":"2025-07-01","issuedBy":{"@type":"Organization","name":"TicketMaster"}}"#;
        let v: Value = serde_json::from_str(json).unwrap();
        let nodes = flatten_nodes(&v);
        let facts = extract_node_facts(Some("Devansh"), nodes[0], "17:42");
        // 1 primary + 1 issuer = 2
        assert_eq!(facts.len(), 2);

        let primary = &facts[0];
        assert_eq!(primary.relationship_type, "has_ticket");
        assert_eq!(primary.object, "TKT-7");
        assert_eq!(primary.event_type, None);

        let issuer = &facts[1];
        assert_eq!(issuer.relationship_type, "issued_by");
        assert_eq!(issuer.object, "TicketMaster");
    }

    // --- ReservationPackage ----------------------------------------------

    #[test]
    fn reservation_package_flattens_sub_reservations() {
        let json = r#"{"@type":"ReservationPackage","subReservation":[{"@type":"FlightReservation","reservationFor":{"@type":"Flight","flightNumber":"100","airline":"BA","departureAirport":{"name":"LHR"},"arrivalAirport":{"name":"JFK"},"departureTime":"2025-08-15T10:00:00Z","arrivalTime":"2025-08-15T14:00:00Z"}},{"@type":"FlightReservation","reservationFor":{"@type":"Flight","flightNumber":"200","airline":"BA","departureAirport":{"name":"JFK"},"arrivalAirport":{"name":"LHR"},"departureTime":"2025-08-25T18:00:00Z","arrivalTime":"2025-08-26T06:00:00Z"}}]}"#;
        let v: Value = serde_json::from_str(json).unwrap();
        let nodes = flatten_nodes(&v);
        let facts = extract_node_facts(Some("Devansh"), nodes[0], "17:42");
        // Each FlightReservation: 1 primary + departs_from + arrives_at + operated_by = 4
        // Two flights → 8 facts total
        assert_eq!(facts.len(), 8);
        assert_eq!(
            facts
                .iter()
                .filter(|f| f.relationship_type == "has_flight")
                .count(),
            2
        );
    }

    // --- Field helpers ----------------------------------------------------

    #[test]
    fn string_or_name_from_string() {
        assert_eq!(
            string_or_name(Some(&Value::String("hello".into()))),
            Some("hello".into())
        );
    }

    #[test]
    fn string_or_name_from_object_with_name() {
        let v: Value = serde_json::from_str(r#"{"@type":"Airline","name":"BA"}"#).unwrap();
        assert_eq!(string_or_name(Some(&v)), Some("BA".into()));
    }

    #[test]
    fn string_or_name_empty_string_returns_none() {
        assert_eq!(string_or_name(Some(&Value::String("  ".into()))), None);
    }

    // --- scalar identifiers (numbers, arrays) ----------------------------

    #[test]
    fn string_or_name_field_accepts_number() {
        let v: Value = serde_json::from_str(r#"{"orderNumber":12345}"#).unwrap();
        let map = v.as_object().unwrap();
        assert_eq!(
            string_or_name_field(map, "orderNumber"),
            Some("12345".into())
        );
    }

    #[test]
    fn string_or_name_field_trims_array_entries() {
        let v: Value = serde_json::from_str(r#"{"name":["  Acme  "]}"#).unwrap();
        let map = v.as_object().unwrap();
        assert_eq!(string_or_name_field(map, "name"), Some("Acme".into()));
    }

    #[test]
    fn string_or_name_field_array_with_number() {
        let v: Value = serde_json::from_str(r#"{"trackingNumber":[98765]}"#).unwrap();
        let map = v.as_object().unwrap();
        assert_eq!(
            string_or_name_field(map, "trackingNumber"),
            Some("98765".into())
        );
    }

    #[test]
    fn airport_name_accepts_numeric_iata_code() {
        let v: Value = serde_json::from_str(r#"{"iataCode":123}"#).unwrap();
        assert_eq!(airport_name(Some(&v)), Some("123".into()));
    }

    #[test]
    fn flight_name_accepts_numeric_flight_number() {
        let json = r#"{"@type":"FlightReservation","reservationFor":{"@type":"Flight","flightNumber":123,"airline":{"@type":"Airline","name":"BA"},"departureAirport":{"name":"LHR"},"arrivalAirport":{"name":"JFK"},"departureTime":"2025-08-15T10:00:00Z","arrivalTime":"2025-08-15T14:00:00Z"}}"#;
        let v: Value = serde_json::from_str(json).unwrap();
        let nodes = flatten_nodes(&v);
        let facts = extract_node_facts(Some("Devansh"), nodes[0], "1:1");
        let primary = &facts[0];
        assert_eq!(primary.object, "BA 123");
    }

    #[test]
    fn order_facts_accept_numeric_order_number() {
        let json = r#"{"@type":"Order","orderNumber":12345,"merchant":{"@type":"Organization","name":"Acme Corp"}}"#;
        let v: Value = serde_json::from_str(json).unwrap();
        let nodes = flatten_nodes(&v);
        let facts = extract_node_facts(Some("Devansh"), nodes[0], "1:1");
        let primary = &facts[0];
        assert_eq!(primary.relationship_type, "has_order");
        assert_eq!(primary.object, "12345");
    }

    #[test]
    fn parcel_delivery_facts_accept_numeric_tracking_number() {
        let json = r#"{"@type":"ParcelDelivery","trackingNumber":987654321,"expectedArrivalFrom":"2025-08-05","expectedArrivalUntil":"2025-08-07","carrier":{"@type":"Organization","name":"DHL"}}"#;
        let v: Value = serde_json::from_str(json).unwrap();
        let nodes = flatten_nodes(&v);
        let facts = extract_node_facts(Some("Devansh"), nodes[0], "1:1");
        let primary = &facts[0];
        assert_eq!(primary.relationship_type, "has_delivery");
        assert_eq!(primary.object, "987654321");
    }

    #[test]
    fn parse_datetime_rfc3339() {
        let v = Value::String("2025-08-15T10:00:00+01:00".into());
        let dt = parse_datetime(&v).unwrap();
        assert_eq!(
            dt,
            DateTime::parse_from_rfc3339("2025-08-15T10:00:00+01:00")
                .unwrap()
                .with_timezone(&Utc)
        );
    }

    #[test]
    fn parse_datetime_date_only() {
        let v = Value::String("2025-08-15".into());
        let dt = parse_datetime(&v).unwrap();
        assert_eq!(
            dt,
            chrono::Utc.with_ymd_and_hms(2025, 8, 15, 0, 0, 0).unwrap()
        );
    }

    #[test]
    fn parse_datetime_naive_treated_as_utc() {
        let v = Value::String("2025-08-15T10:00:00".into());
        let dt = parse_datetime(&v).unwrap();
        assert_eq!(
            dt,
            chrono::Utc.with_ymd_and_hms(2025, 8, 15, 10, 0, 0).unwrap()
        );
    }

    #[test]
    fn parse_datetime_naive_with_fractional_seconds() {
        let v = Value::String("2025-08-15T10:00:00.500".into());
        let dt = parse_datetime(&v).unwrap();
        assert_eq!(
            dt,
            chrono::Utc.with_ymd_and_hms(2025, 8, 15, 10, 0, 0).unwrap()
                + chrono::Duration::milliseconds(500)
        );
    }

    #[test]
    fn parse_datetime_naive_minute_only() {
        let v = Value::String("2025-08-15T10:00".into());
        let dt = parse_datetime(&v).unwrap();
        assert_eq!(
            dt,
            chrono::Utc.with_ymd_and_hms(2025, 8, 15, 10, 0, 0).unwrap()
        );
    }

    #[test]
    fn parse_datetime_invalid_returns_none() {
        let v = Value::String("not a date".into());
        assert!(parse_datetime(&v).is_none());
    }
}
