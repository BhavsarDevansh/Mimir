//! Nominatim JSON response types and parsing helpers.

use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use serde::Deserialize;
use serde_json::Value as JsonValue;

use mimir_core::geocoder::{GeocodeError, GeocodeResult};

#[derive(Debug, Default, Deserialize)]
pub(super) struct NominatimAddress {
    #[serde(default)]
    pub(super) country: Option<String>,
    #[serde(default)]
    pub(super) country_code: Option<String>,
    #[serde(default)]
    pub(super) city: Option<String>,
    #[serde(default)]
    pub(super) town: Option<String>,
    #[serde(default)]
    pub(super) village: Option<String>,
    #[serde(default)]
    pub(super) hamlet: Option<String>,
    #[serde(default)]
    pub(super) municipality: Option<String>,
    #[serde(default)]
    pub(super) county: Option<String>,
    #[serde(default)]
    pub(super) state: Option<String>,
    #[serde(default)]
    pub(super) region: Option<String>,
}

/// A Nominatim place (forward `/search` element). `lat`/`lon` are strings.
#[derive(Debug, Deserialize)]
pub(super) struct NominatimPlace {
    pub(super) lat: String,
    pub(super) lon: String,
    pub(super) display_name: String,
    #[serde(default)]
    pub(super) address: Option<NominatimAddress>,
    #[serde(default)]
    pub(super) namedetails: Option<JsonValue>,
}

impl NominatimPlace {
    pub(super) fn into_result(self) -> Result<GeocodeResult, GeocodeError> {
        let latitude = parse_coord(&self.lat, "lat")?;
        let longitude = parse_coord(&self.lon, "lon")?;
        Ok(place_to_result(
            latitude,
            longitude,
            self.display_name,
            self.address.as_ref(),
            self.namedetails.as_ref(),
        ))
    }
}

/// Nominatim `/reverse` response envelope: a single place *or* an `error`.
#[derive(Debug, Deserialize)]
pub(super) struct NominatimReverseEnvelope {
    #[serde(default)]
    pub(super) error: Option<String>,
    #[serde(default)]
    pub(super) lat: Option<String>,
    #[serde(default)]
    pub(super) lon: Option<String>,
    #[serde(default)]
    pub(super) display_name: Option<String>,
    #[serde(default)]
    pub(super) address: Option<NominatimAddress>,
    #[serde(default)]
    pub(super) namedetails: Option<JsonValue>,
}

/// Build a [`GeocodeResult`] from already-parsed coordinates and the shared
/// Nominatim fields. Coordinate parsing is the caller's responsibility (see
/// [`parse_coord`]) so an unparseable `lat`/`lon` surfaces as
/// [`GeocodeError::Parse`] rather than silently becoming `(0.0, 0.0)`.
pub(super) fn place_to_result(
    latitude: f64,
    longitude: f64,
    display_name: String,
    address: Option<&NominatimAddress>,
    namedetails: Option<&JsonValue>,
) -> GeocodeResult {
    let (country, country_code) = match address {
        Some(addr) => (
            addr.country.clone(),
            addr.country_code.as_ref().map(|c| c.to_lowercase()),
        ),
        None => (None, None),
    };
    let short_name = locality_short_name(address, &display_name);
    let alternative_names = collect_alternative_names(namedetails, &display_name);
    GeocodeResult {
        latitude,
        longitude,
        display_name,
        short_name,
        country,
        country_code,
        alternative_names,
    }
}

/// Derive a canonical, locality-level short name for a reverse-/forward-
/// geocoded place (Phase 3 C2 / #196).
///
/// Returns the most specific populated locality field from the Nominatim
/// `address` block, in descending specificity:
/// `city` → `town` → `village` → `hamlet` → `municipality` → `county` →
/// `state` → `region`. When no locality field is present (e.g. a remote POI
/// with only a country), falls back to the first comma-separated segment of
/// `display_name`, trimmed. `None` only when neither a locality nor a usable
/// display name is reported.
///
/// Using the locality — not the POI `name` — keeps photos taken at different
/// spots in the same city resolving to one `Place` entity so corroboration
/// fires across them, instead of fragmenting into one entity per restaurant
/// / landmark. POI-level detail remains available via `display_name` and
/// `alternative_names` for future vision-tracking queries.
pub(super) fn locality_short_name(
    address: Option<&NominatimAddress>,
    display_name: &str,
) -> Option<String> {
    let locality = address.and_then(|addr| {
        addr.city
            .clone()
            .or_else(|| addr.town.clone())
            .or_else(|| addr.village.clone())
            .or_else(|| addr.hamlet.clone())
            .or_else(|| addr.municipality.clone())
            .or_else(|| addr.county.clone())
            .or_else(|| addr.state.clone())
            .or_else(|| addr.region.clone())
    });
    if let Some(name) = locality {
        let trimmed = name.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    display_name
        .split(',')
        .next()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Parse a Nominatim coordinate string (`lat`/`lon`) into `f64`, mapping a
/// malformed value to [`GeocodeError::Parse`] instead of defaulting to `0.0`.
pub(super) fn parse_coord(value: &str, name: &str) -> Result<f64, GeocodeError> {
    value
        .parse::<f64>()
        .map_err(|e| GeocodeError::Parse(format!("invalid {name} coordinate {value:?}: {e}")))
}

/// Collect non-empty string values from the `namedetails` map, de-duplicated
/// and with the full `display_name` excluded.
pub(super) fn collect_alternative_names(
    namedetails: Option<&JsonValue>,
    display_name: &str,
) -> Vec<String> {
    let map = match namedetails {
        Some(JsonValue::Object(map)) => map,
        _ => return Vec::new(),
    };
    let mut names: Vec<String> = map
        .values()
        .filter_map(|v| v.as_str().map(str::to_string))
        .filter(|s| !s.is_empty() && s != display_name)
        .collect();
    names.sort();
    names.dedup();
    names
}

/// Percent-encode a query parameter value using the vetted `percent-encoding`
/// crate (already a transitive dependency via `reqwest`). `NON_ALPHANUMERIC`
/// encodes everything outside `A-Za-z0-9` (space as `%20`), which Nominatim
/// accepts.
pub(super) fn percent_encode_query(input: &str) -> String {
    utf8_percent_encode(input, NON_ALPHANUMERIC).to_string()
}
