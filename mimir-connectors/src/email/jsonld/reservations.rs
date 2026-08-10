//! Travel reservation JSON-LD extractors: flights, lodging, events, packages.

use mimir_knowledge::models::entity::EntityType;
use mimir_knowledge::models::enums::EventType;
use mimir_knowledge::normalize::NormalizedFact;
use serde_json::Value;

use tracing::debug;

use crate::email::jsonld::facts::jsonld_fact;
use crate::email::jsonld::nodes::{extract_node_facts, flatten_nodes};
use crate::email::jsonld::values::{
    airport_name, flight_name, parse_datetime, scalar_string, string_or_name, string_or_name_field,
};

pub(super) fn flight_reservation_facts(
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
pub(super) fn lodging_reservation_facts(
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
        .and_then(|a| match a {
            // Structured `PostalAddress`: prefer the granular street field.
            Value::Object(map) => string_or_name_field(map, "streetAddress"),
            // schema.org permits `address` as plain text; use it directly.
            _ => scalar_string(a),
        })
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
pub(super) fn event_reservation_facts(
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
pub(super) fn reservation_package_facts(
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
