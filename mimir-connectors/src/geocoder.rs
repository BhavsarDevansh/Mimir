//! OSM Nominatim geocoder backend (Phase 3 S1 / issue #191).
//!
//! Implements [`mimir_core::geocoder::Geocoder`] against the public OpenStreetMap
//! Nominatim API (free, no API key). Forward geocoding hits `/search`; reverse
//! geocoding hits `/reverse`. Throttling reuses the shared F12 [`RateLimiter`]
//! (policy-compliant ≤ 1 req/s via [`RateLimitConfig::nominatim`]); transient
//! HTTP failures (429 / 502 / 503 / 504) and transport errors are retried via
//! [`retry_with_backoff`].
//!
//! # Nominatim usage policy
//!
//! The public instance requires a **descriptive `User-Agent`** and recommends a
//! contact email; both are configurable via [`NominatimConfig`]. The base
//! endpoint is configurable so users can point at a self-hosted Nominatim
//! instance (which the policy actively encourages for heavy use), sidestepping
//! the shared-instance rate ceiling entirely.
//!
//! # Result contract
//!
//! A successful backend response with no match (empty `/search` array, or a
//! `/reverse` `error` payload) yields `Ok(None)`. Transport / decode failures
//! yield `Err(GeocodeError)` so the daemon can log them; they never panic.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use mimir_core::geocoder::{GeocodeError, GeocodeResult, Geocoder};
use serde::Deserialize;
use serde_json::Value as JsonValue;

use crate::rate_limit::{
    RateLimitConfig, RateLimitError, RateLimiter, RetryError, RetryHint, Retryable,
    is_retryable_status, retry_with_backoff,
};

/// Default public Nominatim endpoint (no trailing slash).
pub const DEFAULT_NOMINATIM_ENDPOINT: &str = "https://nominatim.openstreetmap.org";

/// Per-request overall timeout. Nominatim responses are small and fast; this
/// bounds a stalled connection without affecting the token-bucket pacing.
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

/// Default retry budget for a single geocode call (first attempt + retries).
const DEFAULT_MAX_ATTEMPTS: u32 = 3;

/// Configuration for the Nominatim geocoder backend.
#[derive(Debug, Clone)]
pub struct NominatimConfig {
    /// Base endpoint URL (no trailing slash). Defaults to the public instance;
    /// point at a self-hosted instance for heavy use.
    pub endpoint: String,
    /// Descriptive `User-Agent` sent with every request (Nominatim policy
    /// requires identification). Defaults to `Mimir/<version>`.
    pub user_agent: String,
    /// Optional contact email appended to the `User-Agent`. Nominatim
    /// recommends supplying one for the public instance.
    pub contact_email: Option<String>,
    /// Rate-limit + retry/backoff policy. Defaults to [`RateLimitConfig::nominatim`].
    pub rate_limit: RateLimitConfig,
    /// Maximum total attempts (first call + retries) per geocode request.
    pub max_attempts: u32,
    /// Per-request timeout.
    pub request_timeout: Duration,
}

impl NominatimConfig {
    /// Policy-compliant default against the public Nominatim instance.
    pub fn new() -> Self {
        Self {
            endpoint: DEFAULT_NOMINATIM_ENDPOINT.to_string(),
            user_agent: format!("Mimir/{}", env!("CARGO_PKG_VERSION")),
            contact_email: None,
            rate_limit: RateLimitConfig::nominatim(),
            max_attempts: DEFAULT_MAX_ATTEMPTS,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
        }
    }

    /// Builder: set the base endpoint.
    pub fn with_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.endpoint = endpoint.into();
        self
    }

    /// Builder: set the `User-Agent`.
    pub fn with_user_agent(mut self, user_agent: impl Into<String>) -> Self {
        self.user_agent = user_agent.into();
        self
    }

    /// Builder: set the contact email.
    pub fn with_contact_email(mut self, email: impl Into<String>) -> Self {
        self.contact_email = Some(email.into());
        self
    }

    /// Builder: override the rate-limit + retry policy.
    pub fn with_rate_limit(mut self, rate_limit: RateLimitConfig) -> Self {
        self.rate_limit = rate_limit;
        self
    }

    /// Compose the `User-Agent` header value, appending the contact email when
    /// set (Nominatim's recommended form).
    fn user_agent_header(&self) -> String {
        match &self.contact_email {
            Some(email) => format!("{} ({email})", self.user_agent),
            None => self.user_agent.clone(),
        }
    }
}

impl Default for NominatimConfig {
    fn default() -> Self {
        Self::new()
    }
}

/// OSM Nominatim [`Geocoder`] backend.
#[derive(Debug)]
pub struct NominatimGeocoder {
    config: NominatimConfig,
    client: reqwest::Client,
    limiter: Arc<RateLimiter>,
}

impl NominatimGeocoder {
    /// Construct a geocoder from its configuration, building the HTTP client
    /// and rate limiter. Fails only if the client or limiter cannot be built.
    pub fn new(config: NominatimConfig) -> Result<Self, GeocodeError> {
        let client = reqwest::Client::builder()
            .user_agent(config.user_agent_header())
            .timeout(config.request_timeout)
            .build()
            .map_err(|e| GeocodeError::Backend(format!("failed to build HTTP client: {e}")))?;
        let limiter = Arc::new(RateLimiter::new(config.rate_limit.clone()).map_err(map_rate_err)?);
        Ok(Self {
            config,
            client,
            limiter,
        })
    }

    /// Construct from the default public-instance config.
    pub fn with_defaults() -> Result<Self, GeocodeError> {
        Self::new(NominatimConfig::new())
    }

    /// Inject an HTTP client (test seam). The client's `User-Agent` and
    /// timeout are left to the caller.
    pub fn with_http_client(
        config: NominatimConfig,
        client: reqwest::Client,
    ) -> Result<Self, GeocodeError> {
        let limiter = Arc::new(RateLimiter::new(config.rate_limit.clone()).map_err(map_rate_err)?);
        Ok(Self {
            config,
            client,
            limiter,
        })
    }

    /// Reference to the active config (mainly for diagnostics / tests).
    pub fn config(&self) -> &NominatimConfig {
        &self.config
    }

    /// Run a single geocode attempt: acquire a rate-limit token, send the
    /// request, validate the status, and return the raw text body on success.
    async fn attempt(&self, url: &str) -> Result<String, RequestError> {
        // Quota exhaustion is non-retryable: surface as a terminal error so the
        // supervisor / caller pauses rather than hammering Nominatim.
        self.limiter
            .acquire()
            .await
            .map_err(RequestError::RateLimited)?;

        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| RequestError::Network(e.to_string()))?;

        let status = response.status().as_u16();
        if response.status().is_success() {
            return response
                .text()
                .await
                .map_err(|e| RequestError::Network(e.to_string()));
        }

        // For retryable statuses, honour a server `Retry-After` header
        // (seconds) so the backoff layer waits the server's requested period.
        let retry_after = if is_retryable_status(status) {
            response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok())
                .map(Duration::from_secs)
        } else {
            None
        };
        let body = response.text().await.unwrap_or_default();
        Err(RequestError::Status {
            status,
            body,
            retry_after,
        })
    }

    /// Drive `attempt` through the retry/backoff layer and map the outcome to
    /// a `GeocodeError`.
    async fn send_with_retry(&self, url: &str) -> Result<String, GeocodeError> {
        let strategy = self.config.rate_limit.backoff_strategy.clone();
        let max_attempts = self.config.max_attempts.max(1);
        let url_owned = url.to_string();
        let result =
            retry_with_backoff::<RequestError, _, _, String>(&strategy, max_attempts, |_| {
                let url = url_owned.clone();
                async move { self.attempt(&url).await }
            })
            .await;
        match result {
            Ok(body) => Ok(body),
            Err(RetryError::Exhausted { error, .. }) => Err(map_request_error(error)),
            Err(RetryError::Terminal(error)) => Err(map_request_error(error)),
        }
    }
}

#[async_trait]
impl Geocoder for NominatimGeocoder {
    async fn forward(&self, query: &str) -> Result<Option<GeocodeResult>, GeocodeError> {
        let url = format!(
            "{}/search?q={}&format=json&addressdetails=1&namedetails=1",
            self.config.endpoint.trim_end_matches('/'),
            percent_encode_query(query),
        );
        let body = self.send_with_retry(&url).await?;
        let places: Vec<NominatimPlace> =
            serde_json::from_str(&body).map_err(|e| GeocodeError::Parse(e.to_string()))?;
        Ok(places.into_iter().next().map(NominatimPlace::into_result))
    }

    async fn reverse(
        &self,
        latitude: f64,
        longitude: f64,
    ) -> Result<Option<GeocodeResult>, GeocodeError> {
        let url = format!(
            "{}/reverse?lat={}&lon={}&format=json&addressdetails=1&namedetails=1",
            self.config.endpoint.trim_end_matches('/'),
            latitude,
            longitude,
        );
        let body = self.send_with_retry(&url).await?;
        let envelope: NominatimReverseEnvelope =
            serde_json::from_str(&body).map_err(|e| GeocodeError::Parse(e.to_string()))?;
        if envelope.error.is_some() {
            return Ok(None);
        }
        let Some(lat) = envelope.lat.as_deref() else {
            return Ok(None);
        };
        let Some(lon) = envelope.lon.as_deref() else {
            return Ok(None);
        };
        Ok(Some(place_to_result(
            lat,
            lon,
            envelope.display_name.unwrap_or_default(),
            envelope.address.as_ref(),
            envelope.namedetails.as_ref(),
        )))
    }
}

// ---------------------------------------------------------------------------
// Internal: retryable request error
// ---------------------------------------------------------------------------

/// One attempt's failure, classified for [`retry_with_backoff`].
#[derive(Debug)]
enum RequestError {
    /// Rate limiter rejected the attempt (quota exhausted). Non-retryable.
    RateLimited(RateLimitError),
    /// HTTP non-2xx. Retryable for 429/502/503/504 (honouring `retry_after`).
    Status {
        status: u16,
        body: String,
        retry_after: Option<Duration>,
    },
    /// Transport failure (DNS / connect / read). Transient → retryable.
    Network(String),
}

impl Retryable for RequestError {
    fn retry_hint(&self) -> RetryHint {
        match self {
            RequestError::RateLimited(_) => RetryHint::Stop,
            RequestError::Status {
                status,
                retry_after,
                ..
            } => {
                if is_retryable_status(*status) {
                    RetryHint::Retry {
                        retry_after: *retry_after,
                    }
                } else {
                    RetryHint::Stop
                }
            }
            RequestError::Network(_) => RetryHint::Retry { retry_after: None },
        }
    }
}

fn map_request_error(error: RequestError) -> GeocodeError {
    match error {
        RequestError::RateLimited(e) => GeocodeError::RateLimited(e.to_string()),
        RequestError::Status { status, body, .. } => GeocodeError::Status { status, body },
        RequestError::Network(msg) => GeocodeError::Network(msg),
    }
}

/// Map a *construction-time* `RateLimitError` (from `RateLimiter::new`, which
/// only fails with `InvalidConfig` / `InvalidSnapshot`) to a backend error.
/// These are configuration problems, not rate-limiting events, so they surface
/// as [`GeocodeError::Backend`]; the live admission path
/// ([`RequestError::RateLimited`]) still maps to [`GeocodeError::RateLimited`]
/// via [`map_request_error`] so genuine quota exhaustion keeps its label.
fn map_rate_err(error: RateLimitError) -> GeocodeError {
    GeocodeError::Backend(format!("invalid rate-limit config: {error}"))
}

// ---------------------------------------------------------------------------
// Response parsing
// ---------------------------------------------------------------------------

/// Nominatim `address` object subset (classic JSON format).
#[derive(Debug, Deserialize)]
struct NominatimAddress {
    #[serde(default)]
    country: Option<String>,
    #[serde(default)]
    country_code: Option<String>,
}

/// A Nominatim place (forward `/search` element). `lat`/`lon` are strings.
#[derive(Debug, Deserialize)]
struct NominatimPlace {
    lat: String,
    lon: String,
    display_name: String,
    #[serde(default)]
    address: Option<NominatimAddress>,
    #[serde(default)]
    namedetails: Option<JsonValue>,
}

impl NominatimPlace {
    fn into_result(self) -> GeocodeResult {
        place_to_result(
            &self.lat,
            &self.lon,
            self.display_name,
            self.address.as_ref(),
            self.namedetails.as_ref(),
        )
    }
}

/// Nominatim `/reverse` response envelope: a single place *or* an `error`.
#[derive(Debug, Deserialize)]
struct NominatimReverseEnvelope {
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    lat: Option<String>,
    #[serde(default)]
    lon: Option<String>,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    address: Option<NominatimAddress>,
    #[serde(default)]
    namedetails: Option<JsonValue>,
}

/// Build a [`GeocodeResult`] from the shared Nominatim fields, parsing the
/// string `lat`/`lon` into `f64`. Unparseable coordinates default to `0.0`
/// (the reverse path has already validated both `lat` and `lon` presence
/// before calling; the fallback only guards against malformed numeric strings).
fn place_to_result(
    lat: &str,
    lon: &str,
    display_name: String,
    address: Option<&NominatimAddress>,
    namedetails: Option<&JsonValue>,
) -> GeocodeResult {
    let latitude = lat.parse::<f64>().unwrap_or(0.0);
    let longitude = lon.parse::<f64>().unwrap_or(0.0);
    let (country, country_code) = match address {
        Some(addr) => (
            addr.country.clone(),
            addr.country_code.as_ref().map(|c| c.to_lowercase()),
        ),
        None => (None, None),
    };
    let alternative_names = collect_alternative_names(namedetails, &display_name);
    GeocodeResult {
        latitude,
        longitude,
        display_name,
        country,
        country_code,
        alternative_names,
    }
}

/// Collect non-empty string values from the `namedetails` map, de-duplicated
/// and with the full `display_name` excluded.
fn collect_alternative_names(namedetails: Option<&JsonValue>, display_name: &str) -> Vec<String> {
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

/// Percent-encode a query string for a URL query parameter.
fn percent_encode_query(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for &byte in input.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            b' ' => out.push('+'),
            _ => {
                out.push('%');
                out.push_str(&format!("{byte:02X}"));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_encode_query_encodes_spaces_and_special() {
        assert_eq!(
            percent_encode_query("10 Downing St, London"),
            "10+Downing+St%2C+London"
        );
        assert_eq!(percent_encode_query("café"), "caf%C3%A9");
    }

    #[test]
    fn place_to_result_parses_strings_and_country() {
        let addr = NominatimAddress {
            country: Some("United Kingdom".to_string()),
            country_code: Some("GB".to_string()),
        };
        let namedetails = serde_json::json!({
            "name": "London",
            "name:fr": "Londres",
            "alt_name": "London",
        });
        let result = place_to_result(
            "51.5074",
            "-0.1278",
            "London, United Kingdom".to_string(),
            Some(&addr),
            Some(&namedetails),
        );
        assert_eq!(result.latitude, 51.5074);
        assert_eq!(result.longitude, -0.1278);
        assert_eq!(result.country.as_deref(), Some("United Kingdom"));
        assert_eq!(result.country_code.as_deref(), Some("gb"));
        assert!(result.alternative_names.contains(&"Londres".to_string()));
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
}
