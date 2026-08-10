use super::*;

use super::html::{extract_jsonld_blocks, parse_html_attributes};
use super::nodes::{extract_node_facts, flatten_nodes, node_types};
use super::values::{airport_name, parse_datetime, string_or_name, string_or_name_field};
use chrono::{DateTime, TimeZone, Utc};
use mimir_knowledge::models::entity::EntityType;
use mimir_knowledge::models::enums::EventType;
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
fn lodging_reservation_textual_address_emits_location() {
    // schema.org permits `address` as a plain text string. The scalar
    // address must be used directly as the `located_in` object rather
    // than dropped by an `as_object()` guard.
    let json = r#"{"@type":"LodgingReservation","checkinDate":"2025-08-20","checkoutDate":"2025-08-25","reservationFor":{"@type":"LodgingBusiness","name":"Grand Hotel","address":"123 Via Roma, Florence"}}"#;
    let v: Value = serde_json::from_str(json).unwrap();
    let nodes = flatten_nodes(&v);
    let facts = extract_node_facts(None, nodes[0], "1:1");
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].relationship_type, "located_in");
    assert_eq!(facts[0].object, "123 Via Roma, Florence");
    assert_eq!(facts[0].object_type, Some(EntityType::Place));
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
