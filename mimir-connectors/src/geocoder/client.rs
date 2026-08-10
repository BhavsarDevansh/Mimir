//! Nominatim HTTP client: forward/reverse geocoding with retry/backoff.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use mimir_core::geocoder::{GeocodeError, GeocodeResult, Geocoder};

use crate::geocoder::NominatimConfig;
use crate::geocoder::NominatimGeocoder;
use crate::geocoder::parse::{NominatimPlace, NominatimReverseEnvelope};
use crate::geocoder::parse::{parse_coord, percent_encode_query, place_to_result};
use crate::rate_limit::{
    RateLimitError, RateLimiter, RetryError, RetryHint, Retryable, is_retryable_status,
    retry_with_backoff,
};

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
        match places.into_iter().next() {
            Some(place) => Ok(Some(place.into_result()?)),
            None => Ok(None),
        }
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
        let latitude = parse_coord(lat, "lat")?;
        let longitude = parse_coord(lon, "lon")?;
        Ok(Some(place_to_result(
            latitude,
            longitude,
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
