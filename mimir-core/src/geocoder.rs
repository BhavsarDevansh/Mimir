//! Pluggable geocoding abstraction (Phase 3 S1 / issue #191).
//!
//! Forward geocoding resolves a free-text address or place name to
//! coordinates; reverse geocoding resolves a latitude/longitude to a place.
//! The trait lives in `mimir-core` (the shared base layer) rather than
//! `mimir-connectors` because one consumer — the Location Search conversational
//! tool (#98, a `mimir-core` tool) — must be able to name the trait, and
//! `mimir-core` cannot depend on `mimir-connectors` (that would be a cycle).
//! Concrete backends (the OSM Nominatim default, future Mapbox, …) live in
//! `mimir-connectors` and are injected where needed.
//!
//! # Result vs `Option` contract (issue #191 acceptance)
//!
//! Per the issue, "network failure returns `None` gracefully (no panic)". The
//! trait methods return `Result<Option<GeocodeResult>, GeocodeError>` so the
//! two failure modes stay distinguishable:
//!
//! - `Ok(None)` — the backend responded successfully but found no match
//!   (e.g. Nominatim's `[]` array or `error` payload). This is the "graceful
//!   no-result" case.
//! - `Err(GeocodeError)` — a transport, decode, or rate-limit failure.
//!
//! Callers that want the literal issue acceptance ("network failure → None")
//! can map `Err(_)` to `None` (logging first); surfacing the real error by
//! default keeps the daemon observable instead of silently swallowing failures.
//! The "no panic" guarantee holds either way.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Default endpoint for the public OpenStreetMap Nominatim instance.
///
/// This is the single source of truth shared by the compiled-in
/// `geocoder.endpoint` config default and the Nominatim backend
/// (`mimir-connectors`), so the two cannot drift. Pointing at a self-hosted
/// Nominatim instance is encouraged for heavy use (issue #227).
pub const DEFAULT_NOMINATIM_ENDPOINT: &str = "https://nominatim.openstreetmap.org";

/// A single geocoding result, normalised across backends.
///
/// Fields are the subset required by issue #191's acceptance ("lat / lon /
/// country / alternative names") plus `display_name` and the ISO country code,
/// which consumers (#98 location-search tool, the Photos connector place
/// entity) need. Backends map their native representation onto this struct;
/// unknown optional fields are `None`/empty rather than synthetic placeholders.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GeocodeResult {
    /// WGS-84 latitude in decimal degrees.
    pub latitude: f64,
    /// WGS-84 longitude in decimal degrees.
    pub longitude: f64,
    /// Backend's full, human-readable place label (e.g. Nominatim
    /// `display_name`).
    pub display_name: String,
    /// Canonical short name for the place — the locality-level label suitable
    /// for use as a knowledge-graph `Place` entity name (e.g. "Rome", not
    /// "Rome, Metropolitan City of Rome, Italy"). Backends derive this from
    /// the most specific locality field available (city / town / village /
    /// municipality / county / …), falling back to the first segment of
    /// [`display_name`](Self::display_name). `None` only when the backend
    /// reports neither a locality nor a display name. The Photos connector
    /// (Phase 3 C2 / #196) uses this as the object of a `took_photo_at` fact
    /// so photos taken at different spots in the same city resolve to one
    /// place entity and corroborate, instead of fragmenting per POI.
    pub short_name: Option<String>,
    /// Country name in the backend's language, when available.
    pub country: Option<String>,
    /// ISO 3166-1 alpha-2 country code (lowercased), when available.
    pub country_code: Option<String>,
    /// Alternative/known names for the place (e.g. Nominatim `namedetails`),
    /// excluding `display_name`. Empty when the backend reports none.
    pub alternative_names: Vec<String>,
}

/// Errors raised by a [`Geocoder`] backend.
///
/// All variants are non-panic, recoverable failures. "No match found" is
/// **not** an error — it is `Ok(None)` from the trait methods.
#[derive(Debug, Clone, thiserror::Error)]
pub enum GeocodeError {
    /// A network/transport failure (DNS, connection refused, timeout, TLS).
    #[error("network error: {0}")]
    Network(String),
    /// The backend returned an unexpected HTTP status (non-2xx, excluding
    /// rate-limiting handled by the retry layer).
    #[error("geocoder returned status {status}: {body}")]
    Status { status: u16, body: String },
    /// The response body could not be decoded into the expected shape.
    #[error("invalid geocoder response: {0}")]
    Parse(String),
    /// The configured rate limit / daily quota was exhausted before the
    /// request could be admitted.
    #[error("rate limited: {0}")]
    RateLimited(String),
    /// Any other backend-specific failure not covered above.
    #[error("geocoder backend error: {0}")]
    Backend(String),
}

/// Object-safe, async, pluggable geocoder.
///
/// Implementations are stored as `Arc<dyn Geocoder>` so a single instance can be
/// shared across the Photos connector (C2), the entity-locations write path
/// (S3), and the Location Search tool (#98).
#[async_trait]
pub trait Geocoder: Send + Sync + std::fmt::Debug {
    /// Forward geocode: resolve `query` (an address or place name) to
    /// coordinates. Returns `Ok(None)` when the backend finds no match.
    async fn forward(&self, query: &str) -> Result<Option<GeocodeResult>, GeocodeError>;

    /// Reverse geocode: resolve `(latitude, longitude)` to a place. Returns
    /// `Ok(None)` when the backend finds no match.
    async fn reverse(
        &self,
        latitude: f64,
        longitude: f64,
    ) -> Result<Option<GeocodeResult>, GeocodeError>;
}

// ---------------------------------------------------------------------------
// Mock backend (test injection / consumer wiring)
// ---------------------------------------------------------------------------

/// A programmable in-memory [`Geocoder`] for tests and consumer wiring.
///
/// Each direction consults a shared `MockState` behind a `std::sync::Mutex`
/// so the mock is `Send + Sync`, clones cheaply, and can be (re)configured
/// synchronously via the builder methods. A `std::sync::Mutex` (not a tokio
/// mutex) is used deliberately so the [`with_forward`](Self::with_forward) /
/// [`with_reverse`](Self::with_reverse) builders stay synchronous and chainable;
/// the trait methods only hold the guard briefly to clone a value out and
/// never await while holding it. The default state returns `Ok(None)` for both
/// directions.
#[derive(Debug, Clone, Default)]
pub struct MockGeocoder {
    state: std::sync::Arc<std::sync::Mutex<MockState>>,
}

#[derive(Debug, Default)]
struct MockState {
    forward_result: Option<Result<Option<GeocodeResult>, GeocodeError>>,
    reverse_result: Option<Result<Option<GeocodeResult>, GeocodeError>>,
}

impl MockGeocoder {
    /// Create a mock that returns `Ok(None)` for both directions.
    pub fn new() -> Self {
        Self::default()
    }

    /// Configure the result returned by [`Geocoder::forward`]. Composes with
    /// [`with_reverse`](Self::with_reverse): chaining preserves both settings.
    pub fn with_forward(self, result: Result<Option<GeocodeResult>, GeocodeError>) -> Self {
        self.state
            .lock()
            .expect("mock state poisoned")
            .forward_result = Some(result);
        self
    }

    /// Configure the result returned by [`Geocoder::reverse`]. Composes with
    /// [`with_forward`](Self::with_forward): chaining preserves both settings.
    pub fn with_reverse(self, result: Result<Option<GeocodeResult>, GeocodeError>) -> Self {
        self.state
            .lock()
            .expect("mock state poisoned")
            .reverse_result = Some(result);
        self
    }
}

#[async_trait]
impl Geocoder for MockGeocoder {
    async fn forward(&self, _query: &str) -> Result<Option<GeocodeResult>, GeocodeError> {
        // Brief synchronous lock: clone the result out, then drop the guard
        // before returning. No await is held across the lock.
        let state = self.state.lock().expect("mock state poisoned");
        match &state.forward_result {
            Some(result) => result.clone(),
            None => Ok(None),
        }
    }

    async fn reverse(
        &self,
        _latitude: f64,
        _longitude: f64,
    ) -> Result<Option<GeocodeResult>, GeocodeError> {
        let state = self.state.lock().expect("mock state poisoned");
        match &state.reverse_result {
            Some(result) => result.clone(),
            None => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_result() -> GeocodeResult {
        GeocodeResult {
            latitude: 51.5074,
            longitude: -0.1278,
            display_name: "London, Greater London, England, United Kingdom".to_string(),
            short_name: Some("London".to_string()),
            country: Some("United Kingdom".to_string()),
            country_code: Some("gb".to_string()),
            alternative_names: vec!["Londres".to_string(), "Londra".to_string()],
        }
    }

    #[tokio::test]
    async fn mock_forward_returns_configured_result() {
        let g = MockGeocoder::new().with_forward(Ok(Some(sample_result())));
        let got = g.forward("London").await.unwrap();
        assert_eq!(got, Some(sample_result()));
    }

    #[tokio::test]
    async fn mock_reverse_returns_configured_result() {
        let g = MockGeocoder::new().with_reverse(Ok(Some(sample_result())));
        let got = g.reverse(51.5074, -0.1278).await.unwrap();
        assert_eq!(got, Some(sample_result()));
    }

    #[tokio::test]
    async fn mock_returns_none_by_default() {
        let g = MockGeocoder::new();
        assert_eq!(g.forward("nowhere").await.unwrap(), None);
        assert_eq!(g.reverse(0.0, 0.0).await.unwrap(), None);
    }

    #[tokio::test]
    async fn mock_surfaces_configured_error() {
        let g = MockGeocoder::new().with_forward(Err(GeocodeError::Network("timeout".to_string())));
        let err = g.forward("x").await.unwrap_err();
        assert!(matches!(err, GeocodeError::Network(_)));
    }

    #[tokio::test]
    async fn mock_chains_forward_and_reverse() {
        // Builders compose: configuring reverse must not erase forward.
        let g = MockGeocoder::new()
            .with_forward(Ok(Some(sample_result())))
            .with_reverse(Ok(None));
        assert_eq!(g.forward("London").await.unwrap(), Some(sample_result()));
        assert_eq!(g.reverse(0.0, 0.0).await.unwrap(), None);
    }

    #[test]
    fn geocode_result_serde_round_trips() {
        let result = sample_result();
        let json = serde_json::to_string(&result).unwrap();
        let back: GeocodeResult = serde_json::from_str(&json).unwrap();
        assert_eq!(result, back);
    }
}
