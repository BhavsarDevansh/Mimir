//! JSON-LD node traversal and type dispatch.

use crate::email::jsonld::facts::{order_facts, parcel_delivery_facts, ticket_facts};
use crate::email::jsonld::reservations::{
    event_reservation_facts, flight_reservation_facts, lodging_reservation_facts,
    reservation_package_facts,
};
use mimir_knowledge::normalize::NormalizedFact;
use serde_json::Value;
use tracing::debug;

pub(super) fn flatten_nodes(value: &Value) -> Vec<&serde_json::Map<String, Value>> {
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
pub(super) fn node_types(node: &serde_json::Map<String, Value>) -> Vec<String> {
    match node.get("@type") {
        Some(Value::String(s)) => vec![s.clone()],
        Some(Value::Array(arr)) => arr
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect(),
        _ => Vec::new(),
    }
}

pub(super) fn extract_node_facts(
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
