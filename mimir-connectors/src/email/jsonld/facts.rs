//! Commerce JSON-LD extractors: orders, parcel deliveries, and tickets.

use chrono::{DateTime, Utc};
use mimir_knowledge::models::entity::EntityType;
use mimir_knowledge::models::enums::{EventType, RecurrenceType};
use mimir_knowledge::models::source::ExtractionMethod;
use mimir_knowledge::normalize::NormalizedFact;
use serde_json::Value;

use crate::email::jsonld::values::{parse_datetime, string_or_name, string_or_name_field};
use crate::fact::connector_fact;
use tracing::debug;

#[allow(clippy::too_many_arguments)]
pub(super) fn jsonld_fact(
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
    connector_fact(
        subject,
        subject_type,
        relationship_type,
        object,
        true,
        object_type,
        valid_from,
        valid_until,
        RecurrenceType::None,
        raw_ref,
        Some(ExtractionMethod::StructuredParse),
        event_type,
        None,
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
pub(super) fn order_facts(
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
pub(super) fn parcel_delivery_facts(
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
pub(super) fn ticket_facts(
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
