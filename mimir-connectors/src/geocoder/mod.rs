//! OSM Nominatim geocoder backend (Phase 3 S1 / issue #191).
//!
//! Implements [`mimir_core::geocoder::Geocoder`] against the public OpenStreetMap
//! Nominatim API (free, no API key). Forward geocoding hits `/search`; reverse
//! geocoding hits `/reverse`. Throttling reuses the shared F12 [`RateLimiter`]
//! (policy-compliant ≤ 1 req/s via [`RateLimitConfig::nominatim`]); transient
//! HTTP failures (429 / 502 / 503 / 504) and transport errors are retried via
//! [`retry_with_backoff`](crate::rate_limit::retry_with_backoff).
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

use crate::rate_limit::{RateLimitConfig, RateLimiter};

mod client;
mod parse;
#[cfg(test)]
mod tests;

/// Default endpoint for the public OpenStreetMap Nominatim instance.
///
/// Re-exported from `mimir-core` so the compiled-in `geocoder.endpoint` config
/// default and the backend share one source of truth (issue #227).
pub use mimir_core::geocoder::DEFAULT_NOMINATIM_ENDPOINT;

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

impl From<&mimir_core::config::GeocoderConfig> for NominatimConfig {
    /// Map the user-facing `geocoder` config section onto the backend config,
    /// keeping the policy-compliant defaults for everything the section does
    /// not expose (user agent, rate limit, retry budget, timeout).
    fn from(config: &mimir_core::config::GeocoderConfig) -> Self {
        let mut nominatim = Self::new().with_endpoint(config.endpoint.clone());
        if let Some(email) = &config.contact_email {
            nominatim = nominatim.with_contact_email(email.clone());
        }
        nominatim
    }
}

/// OSM Nominatim [`Geocoder`](mimir_core::geocoder::Geocoder) backend.
#[derive(Debug)]
pub struct NominatimGeocoder {
    config: NominatimConfig,
    client: reqwest::Client,
    limiter: Arc<RateLimiter>,
}
