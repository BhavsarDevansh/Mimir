//! Nominatim geocoder tests.

use super::*;
use crate::geocoder::parse::{
    NominatimAddress, NominatimReverseEnvelope, collect_alternative_names, locality_short_name,
    parse_coord, percent_encode_query, place_to_result,
};
use mimir_core::geocoder::GeocodeError;

#[test]
fn percent_encode_query_encodes_spaces_and_special() {
    assert_eq!(
        percent_encode_query("10 Downing St, London"),
        "10%20Downing%20St%2C%20London"
    );
    assert_eq!(percent_encode_query("café"), "caf%C3%A9");
}

#[test]
fn place_to_result_builds_result_with_country() {
    let addr = NominatimAddress {
        country: Some("United Kingdom".to_string()),
        country_code: Some("GB".to_string()),
        city: Some("London".to_string()),
        ..Default::default()
    };
    let namedetails = serde_json::json!({
        "name": "London",
        "name:fr": "Londres",
        "alt_name": "London",
    });
    let result = place_to_result(
        51.5074,
        -0.1278,
        "London, United Kingdom".to_string(),
        Some(&addr),
        Some(&namedetails),
    );
    assert_eq!(result.latitude, 51.5074);
    assert_eq!(result.longitude, -0.1278);
    assert_eq!(result.country.as_deref(), Some("United Kingdom"));
    assert_eq!(result.country_code.as_deref(), Some("gb"));
    // Locality (city) wins over the display_name first segment.
    assert_eq!(result.short_name.as_deref(), Some("London"));
    assert!(result.alternative_names.contains(&"Londres".to_string()));
}

#[test]
fn locality_short_name_prefers_city_over_town() {
    let addr = NominatimAddress {
        city: Some("Rome".to_string()),
        town: Some("Ignored".to_string()),
        ..Default::default()
    };
    assert_eq!(
        locality_short_name(Some(&addr), "Rome, Italy"),
        Some("Rome".to_string())
    );
}

#[test]
fn locality_short_name_falls_through_specificity_chain() {
    // No city/town/village/hamlet/municipality/county -> state wins.
    let addr = NominatimAddress {
        state: Some("Texas".to_string()),
        region: Some("Ignored".to_string()),
        ..Default::default()
    };
    assert_eq!(
        locality_short_name(Some(&addr), "Somewhere, Texas, USA"),
        Some("Texas".to_string())
    );
}

#[test]
fn locality_short_name_falls_back_to_display_name_segment() {
    // POI with no locality field: use the first display_name segment.
    let addr = NominatimAddress {
        country: Some("Italy".to_string()),
        ..Default::default()
    };
    assert_eq!(
        locality_short_name(Some(&addr), "Trattoria Luzzi, Rome, Italy"),
        Some("Trattoria Luzzi".to_string())
    );
}

#[test]
fn locality_short_name_none_when_display_name_empty() {
    assert_eq!(locality_short_name(None, ""), None);
}

#[test]
fn locality_short_name_trims_and_skips_blank_locality() {
    // A whitespace-only locality is ignored; fall back to display_name.
    let addr = NominatimAddress {
        city: Some("   ".to_string()),
        ..Default::default()
    };
    assert_eq!(
        locality_short_name(Some(&addr), "Rome, Italy"),
        Some("Rome".to_string())
    );
}

#[test]
fn parse_coord_returns_err_on_garbage() {
    let err = parse_coord("not-a-number", "lat").unwrap_err();
    assert!(matches!(err, GeocodeError::Parse(_)), "got {err:?}");
}

#[test]
fn collect_alternative_names_dedupes_and_skips_display_name() {
    let namedetails = serde_json::json!({
        "name": "Roma",
        "name:de": "Rom",
        "alt_name": "Roma",
    });
    let names = collect_alternative_names(Some(&namedetails), "Roma, Italy");
    assert_eq!(names, vec!["Rom".to_string(), "Roma".to_string()]);
}

#[test]
fn collect_alternative_names_handles_absent_map() {
    assert!(collect_alternative_names(None, "x").is_empty());
    assert!(collect_alternative_names(Some(&serde_json::json!(42)), "x").is_empty());
}

#[test]
fn reverse_envelope_error_yields_none_via_caller() {
    let body = r#"{"error": "Unable to geocode"}"#;
    let env: NominatimReverseEnvelope = serde_json::from_str(body).unwrap();
    assert!(env.error.is_some());
    assert!(env.lat.is_none());
}

#[test]
fn config_user_agent_includes_email_when_set() {
    let cfg = NominatimConfig::new().with_contact_email("dev@example.com");
    assert!(cfg.user_agent_header().contains("(dev@example.com)"));
}
