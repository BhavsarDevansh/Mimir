//! Integration tests for the Nominatim geocoder backend (Phase 3 S1 / #191).
//!
//! These exercise the real HTTP path against a `wiremock` mock server,
//! covering the issue's acceptance: forward + reverse work, results carry
//! lat/lon/country/alternative names, rate limiting reuses the F12 limiter,
//! 429/503 retry with backoff, and network failure is surfaced (no panic).

use std::time::Duration;

use mimir_connectors::rate_limit::{BackoffStrategy, RateLimitConfig};
use mimir_connectors::{NominatimConfig, NominatimGeocoder};
use mimir_core::geocoder::{GeocodeError, Geocoder};
use wiremock::matchers::{method, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// A permissive rate-limit config so parsing/retry tests run in milliseconds
/// rather than waiting on the 1 req/s Nominatim policy.
fn fast_limits() -> RateLimitConfig {
    RateLimitConfig {
        requests_per_second: 100.0,
        burst_size: 100,
        daily_quota: None,
        backoff_strategy: BackoffStrategy::Exponential {
            base: Duration::from_millis(1),
            max: Duration::from_millis(10),
            jitter: Duration::ZERO,
        },
    }
}

fn config_for(server: &MockServer) -> NominatimConfig {
    NominatimConfig::new()
        .with_endpoint(server.uri())
        .with_rate_limit(fast_limits())
}

/// Sample forward `/search` array with address + namedetails.
const SEARCH_LONDON: &str = r#"[
  {
    "place_id": 100149,
    "licence": "Data © OpenStreetMap contributors, ODbL 1.0.",
    "osm_type": "node",
    "osm_id": "107775",
    "lat": "51.5073219",
    "lon": "-0.1276474",
    "display_name": "London, Greater London, England, United Kingdom",
    "class": "place",
    "type": "city",
    "importance": 0.965,
    "address": {
      "city": "London",
      "state": "England",
      "country": "United Kingdom",
      "country_code": "gb"
    },
    "namedetails": {
      "name": "London",
      "name:fr": "Londres",
      "name:de": "London"
    }
  }
]"#;

const REVERSE_LONDON: &str = r#"{
  "place_id": 100149,
  "lat": "51.5073219",
  "lon": "-0.1276474",
  "display_name": "London, Greater London, England, United Kingdom",
  "address": {
    "city": "London",
    "country": "United Kingdom",
    "country_code": "GB"
  },
  "namedetails": {
    "name": "London"
  }
}"#;

#[tokio::test]
async fn forward_geocode_parses_first_result() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(query_param("q", "London"))
        .respond_with(ResponseTemplate::new(200).set_body_string(SEARCH_LONDON))
        .expect(1)
        .mount(&server)
        .await;

    let geocoder = NominatimGeocoder::new(config_for(&server)).unwrap();
    let result = geocoder.forward("London").await.unwrap().unwrap();
    assert_eq!(result.latitude, 51.5073219);
    assert_eq!(result.longitude, -0.1276474);
    assert_eq!(
        result.display_name,
        "London, Greater London, England, United Kingdom"
    );
    assert_eq!(result.country.as_deref(), Some("United Kingdom"));
    assert_eq!(result.country_code.as_deref(), Some("gb"));
    // "London" equals the namedetails `name` but differs from display_name, so
    // it is kept; "Londres" is the alternative name we care about.
    assert!(result.alternative_names.contains(&"Londres".to_string()));
}

#[tokio::test]
async fn forward_geocode_empty_array_yields_none() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(query_param("q", "zzzznotaplace"))
        .respond_with(ResponseTemplate::new(200).set_body_string("[]"))
        .expect(1)
        .mount(&server)
        .await;

    let geocoder = NominatimGeocoder::new(config_for(&server)).unwrap();
    assert_eq!(geocoder.forward("zzzznotaplace").await.unwrap(), None);
}

#[tokio::test]
async fn reverse_geocode_parses_single_place() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(query_param("lat", "51.5073219"))
        .and(query_param("lon", "-0.1276474"))
        .respond_with(ResponseTemplate::new(200).set_body_string(REVERSE_LONDON))
        .expect(1)
        .mount(&server)
        .await;

    let geocoder = NominatimGeocoder::new(config_for(&server)).unwrap();
    let result = geocoder
        .reverse(51.5073219, -0.1276474)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result.latitude, 51.5073219);
    assert_eq!(result.longitude, -0.1276474);
    assert_eq!(result.country.as_deref(), Some("United Kingdom"));
    assert_eq!(result.country_code.as_deref(), Some("gb"));
}

#[tokio::test]
async fn reverse_geocode_error_payload_yields_none() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(r#"{"error": "Unable to geocode"}"#),
        )
        .expect(1)
        .mount(&server)
        .await;

    let geocoder = NominatimGeocoder::new(config_for(&server)).unwrap();
    assert_eq!(geocoder.reverse(0.0, 0.0).await.unwrap(), None);
}

#[tokio::test]
async fn retryable_429_then_success_returns_result() {
    let server = MockServer::start().await;
    // First attempt: 429. Second attempt: 200 with the result.
    Mock::given(method("GET"))
        .and(query_param("q", "London"))
        .respond_with(ResponseTemplate::new(429))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(query_param("q", "London"))
        .respond_with(ResponseTemplate::new(200).set_body_string(SEARCH_LONDON))
        .expect(1)
        .mount(&server)
        .await;

    let geocoder = NominatimGeocoder::new(config_for(&server)).unwrap();
    let result = geocoder.forward("London").await.unwrap().unwrap();
    assert!((result.latitude - 51.5073219).abs() < 1e-6);
}

#[tokio::test]
async fn persistent_503_exhausts_retries_and_surfaces_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;

    let mut config = config_for(&server);
    config.max_attempts = 2;
    let geocoder = NominatimGeocoder::new(config).unwrap();
    let err = geocoder.forward("London").await.unwrap_err();
    assert!(
        matches!(err, GeocodeError::Status { status: 503, .. }),
        "got {err:?}"
    );
}

#[tokio::test]
async fn non_retryable_404_surfaces_immediately_without_retry() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(404).set_body_string("not found"))
        .expect(1)
        .mount(&server)
        .await;

    let geocoder = NominatimGeocoder::new(config_for(&server)).unwrap();
    let err = geocoder.forward("London").await.unwrap_err();
    match err {
        GeocodeError::Status { status, body } => {
            assert_eq!(status, 404);
            assert_eq!(body, "not found");
        }
        other => panic!("expected Status, got {other:?}"),
    }
}

#[tokio::test]
async fn connection_refused_surfaces_network_error_no_panic() {
    // Point at a port that is closed: a transport failure, not a panic.
    let config = NominatimConfig::new()
        .with_endpoint("http://127.0.0.1:1")
        .with_rate_limit(fast_limits());
    let geocoder = NominatimGeocoder::new(config).unwrap();
    let err = geocoder.forward("London").await.unwrap_err();
    assert!(matches!(err, GeocodeError::Network(_)), "got {err:?}");
}

#[tokio::test]
async fn rate_limiter_throttles_consecutive_requests() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("[]"))
        .expect(2)
        .mount(&server)
        .await;

    // Tight policy: burst 1, 2 req/s ⇒ the second call waits ~500ms.
    let config = NominatimConfig::new()
        .with_endpoint(server.uri())
        .with_rate_limit(RateLimitConfig {
            requests_per_second: 2.0,
            burst_size: 1,
            daily_quota: None,
            backoff_strategy: BackoffStrategy::Exponential {
                base: Duration::from_millis(1),
                max: Duration::from_millis(10),
                jitter: Duration::ZERO,
            },
        });
    let geocoder = NominatimGeocoder::new(config).unwrap();
    let start = std::time::Instant::now();
    geocoder.forward("a").await.unwrap();
    geocoder.forward("b").await.unwrap();
    let elapsed = start.elapsed();
    assert!(
        elapsed >= Duration::from_millis(400),
        "expected throttling, elapsed {elapsed:?}"
    );
}

#[tokio::test]
async fn invalid_rate_config_surfaces_as_backend_error_not_rate_limited() {
    use wiremock::MockServer;
    let server = MockServer::start().await;
    let bad = NominatimConfig::new()
        .with_endpoint(server.uri())
        .with_rate_limit(RateLimitConfig {
            requests_per_second: 0.0,
            burst_size: 1,
            daily_quota: None,
            backoff_strategy: BackoffStrategy::Exponential {
                base: Duration::from_millis(1),
                max: Duration::from_millis(10),
                jitter: Duration::ZERO,
            },
        });
    // Construction-time config failure -> Backend, not RateLimited.
    let err = NominatimGeocoder::new(bad).unwrap_err();
    assert!(
        matches!(err, GeocodeError::Backend(_)),
        "expected Backend, got {err:?}"
    );
}

#[tokio::test]
async fn forward_unparseable_coordinate_surfaces_parse_error() {
    let server = MockServer::start().await;
    let body = r#"[{"lat":"not-a-number","lon":"-0.1276","display_name":"x"}]"#;
    Mock::given(method("GET"))
        .and(query_param("q", "weird"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .expect(1)
        .mount(&server)
        .await;

    let geocoder = NominatimGeocoder::new(config_for(&server)).unwrap();
    let err = geocoder.forward("weird").await.unwrap_err();
    assert!(matches!(err, GeocodeError::Parse(_)), "got {err:?}");
}
