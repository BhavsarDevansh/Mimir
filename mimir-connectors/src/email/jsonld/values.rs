//! Value extraction helpers shared by the JSON-LD extractors.

use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use serde_json::Value;

pub(super) fn scalar_string(value: &Value) -> Option<String> {
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
pub(super) fn string_or_name(value: Option<&Value>) -> Option<String> {
    let value = value?;
    match value {
        Value::String(_) => scalar_string(value),
        Value::Object(map) => string_or_name_field(map, "name"),
        _ => None,
    }
}

/// Read a `name` (or alternate) field from a JSON object as a non-empty
/// trimmed string.
pub(super) fn string_or_name_field(
    map: &serde_json::Map<String, Value>,
    field: &str,
) -> Option<String> {
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
pub(super) fn airport_name(value: Option<&Value>) -> Option<String> {
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
pub(super) fn flight_name(flight: &serde_json::Map<String, Value>) -> Option<String> {
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
pub(super) fn parse_datetime(value: &Value) -> Option<DateTime<Utc>> {
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
