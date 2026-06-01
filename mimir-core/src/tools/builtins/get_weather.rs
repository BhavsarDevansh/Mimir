use super::super::{Tool, ToolError, ToolOutput, ToolPermission};
use async_trait::async_trait;
use serde_json::Value;
use urlencoding::encode;

const DEFAULT_BASE_URL: &str = "https://wttr.in";

/// Fetches current weather conditions and up to a 3-day forecast for a given
/// location using wttr.in. All measurements are metric-only.
pub struct GetWeatherTool {
    client: reqwest::Client,
    base_url: String,
}

impl Default for GetWeatherTool {
    fn default() -> Self {
        Self::new()
    }
}

impl GetWeatherTool {
    /// Create a new `GetWeatherTool` using the production wttr.in endpoint.
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url: DEFAULT_BASE_URL.to_string(),
        }
    }

    /// Create a `GetWeatherTool` with a custom base URL (useful for testing).
    pub fn with_base_url(base_url: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url: base_url.into(),
        }
    }

    /// Build the wttr.in URL for a location with JSON output.
    /// Properly percent-encodes the location string to handle spaces,
    /// reserved characters, and non-ASCII characters.
    fn build_url(&self, location: &str) -> String {
        let encoded = encode(location);
        format!("{}/{encoded}?format=j1", self.base_url)
    }

    /// Extract a string field from a JSON object, falling back to "unknown".
    fn extract_str<'a>(obj: &'a Value, key: &str) -> &'a str {
        obj.get(key).and_then(|v| v.as_str()).unwrap_or("unknown")
    }

    /// Extract the first `value` from a nested array object (e.g. `weatherDesc`).
    fn extract_nested_value<'a>(obj: &'a Value, key: &str) -> &'a str {
        obj.get(key)
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
            .and_then(|v| v.get("value"))
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
    }

    /// Build a concise current-conditions object from the wttr.in JSON response.
    fn parse_current_condition(body: &str) -> Result<Value, ToolError> {
        let parsed: Value = serde_json::from_str(body).map_err(|e| {
            ToolError::execution_failed(
                "get_weather",
                format!("failed to parse wttr.in response: {e}"),
            )
        })?;

        let current = parsed
            .get("current_condition")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
            .ok_or_else(|| {
                ToolError::execution_failed("get_weather", "missing current_condition in response")
            })?;

        Ok(serde_json::json!({
            "temperature_c": Self::extract_str(current, "temp_C"),
            "feels_like_c": Self::extract_str(current, "FeelsLikeC"),
            "description": Self::extract_nested_value(current, "weatherDesc"),
            "humidity_percent": Self::extract_str(current, "humidity"),
            "wind_kmph": Self::extract_str(current, "windspeedKmph"),
            "wind_direction": Self::extract_str(current, "winddir16Point"),
            "uv_index": Self::extract_str(current, "uvIndex"),
            "visibility_km": Self::extract_str(current, "visibility"),
            "pressure_mb": Self::extract_str(current, "pressure"),
        }))
    }

    /// Extract a representative midday hourly entry from a forecast day.
    /// Falls back to the first available hourly entry.
    fn get_midday_hourly(day: &Value) -> Option<&Value> {
        day.get("hourly")
            .and_then(|v| v.as_array())
            .and_then(|arr| {
                arr.iter()
                    .find(|h| h.get("time").and_then(|t| t.as_str()) == Some("1200"))
                    .or_else(|| arr.first())
            })
    }

    /// Build a concise forecast-day object from a single wttr.in `weather` entry.
    fn parse_forecast_day(day: &Value) -> Value {
        let hourly = Self::get_midday_hourly(day);

        serde_json::json!({
            "date": Self::extract_str(day, "date"),
            "min_temp_c": Self::extract_str(day, "mintempC"),
            "max_temp_c": Self::extract_str(day, "maxtempC"),
            "avg_temp_c": Self::extract_str(day, "avgtempC"),
            "description": hourly.map(|h| Self::extract_nested_value(h, "weatherDesc")).unwrap_or("unknown"),
            "chance_of_rain_percent": hourly.and_then(|h| h.get("chanceofrain")).and_then(|v| v.as_str()).unwrap_or("unknown"),
            "chance_of_snow_percent": hourly.and_then(|h| h.get("chanceofsnow")).and_then(|v| v.as_str()).unwrap_or("unknown"),
            "uv_index": hourly.and_then(|h| h.get("uvIndex")).and_then(|v| v.as_str()).unwrap_or("unknown"),
        })
    }

    /// Build a forecast array from the wttr.in JSON response.
    fn parse_forecast(body: &str) -> Result<Vec<Value>, ToolError> {
        let parsed: Value = serde_json::from_str(body).map_err(|e| {
            ToolError::execution_failed(
                "get_weather",
                format!("failed to parse wttr.in response: {e}"),
            )
        })?;

        let weather = parsed
            .get("weather")
            .and_then(|v| v.as_array())
            .ok_or_else(|| {
                ToolError::execution_failed("get_weather", "missing weather forecast in response")
            })?;

        Ok(weather.iter().map(Self::parse_forecast_day).collect())
    }
}

#[async_trait]
impl Tool for GetWeatherTool {
    fn name(&self) -> &str {
        "get_weather"
    }

    fn description(&self) -> &str {
        "Fetches current weather conditions and up to a 3-day forecast for a \
given location using wttr.in. All measurements are metric-only. Returns \
temperature (°C), conditions, humidity, wind (km/h), UV index, visibility \
(km), pressure (mb), and forecast summaries with rain probability."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "location": {
                    "type": "string",
                    "description": "The location to get weather for. Can be a city name, airport code, or coordinates."
                },
                "date": {
                    "type": "string",
                    "description": "Optional. Use 'current' for current conditions only, or a YYYY-MM-DD date for a specific forecast day. Omit to get current conditions plus all available forecast days."
                }
            },
            "required": ["location"],
            "additionalProperties": false,
        })
    }

    fn permission(&self) -> ToolPermission {
        ToolPermission::Auto
    }

    async fn execute(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let location = args
            .get("location")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ToolError::invalid_arguments("get_weather", "missing 'location' argument")
            })?;

        if location.trim().is_empty() {
            return Err(ToolError::invalid_arguments(
                "get_weather",
                "'location' must not be empty",
            ));
        }

        let date_arg = args.get("date").and_then(|v| v.as_str());

        let url = self.build_url(location);
        let user_agent = format!("Mimir/{}", env!("CARGO_PKG_VERSION"));

        let response = self
            .client
            .get(&url)
            .header("User-Agent", user_agent)
            .timeout(std::time::Duration::from_secs(15))
            .send()
            .await
            .map_err(|e| {
                ToolError::execution_failed("get_weather", format!("HTTP request failed: {e}"))
            })?;

        let status = response.status();
        let body = response.text().await.map_err(|e| {
            ToolError::execution_failed("get_weather", format!("failed to read response body: {e}"))
        })?;

        if !status.is_success() {
            return Err(ToolError::execution_failed(
                "get_weather",
                format!("wttr.in returned HTTP {status}: {body}"),
            ));
        }

        // wttr.in returns a plain-text error for unknown locations even with 200 OK
        if body.contains("Unknown location") || body.contains("Not Found") {
            return Err(ToolError::execution_failed(
                "get_weather",
                format!("location '{location}' not found by wttr.in"),
            ));
        }

        match date_arg {
            Some("current") => {
                let current = Self::parse_current_condition(&body)?;
                Ok(ToolOutput {
                    result: Some(current),
                    ..Default::default()
                })
            }
            Some(specific_date) => {
                let forecast = Self::parse_forecast(&body)?;
                let day = forecast
                    .iter()
                    .find(|d| d.get("date").and_then(|v| v.as_str()) == Some(specific_date));

                match day {
                    Some(d) => Ok(ToolOutput {
                        result: Some(d.clone()),
                        ..Default::default()
                    }),
                    None => {
                        let available = forecast
                            .iter()
                            .filter_map(|d| d.get("date").and_then(|v| v.as_str()))
                            .collect::<Vec<_>>()
                            .join(", ");
                        Err(ToolError::execution_failed(
                            "get_weather",
                            format!(
                                "forecast not available for '{specific_date}'. Available dates: {available}"
                            ),
                        ))
                    }
                }
            }
            None => {
                let mut current = Self::parse_current_condition(&body)?;
                if let Ok(forecast) = Self::parse_forecast(&body) {
                    if !forecast.is_empty() {
                        current["forecast"] = serde_json::json!(forecast);
                    }
                }
                Ok(ToolOutput {
                    result: Some(current),
                    ..Default::default()
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MOCK_BODY: &str = r#"{
        "current_condition": [{
            "temp_C": "18",
            "temp_F": "64",
            "FeelsLikeC": "17",
            "weatherDesc": [{"value": "Partly cloudy"}],
            "humidity": "65",
            "windspeedKmph": "12",
            "winddir16Point": "SW",
            "uvIndex": "4",
            "visibility": "10",
            "pressure": "1015"
        }],
        "weather": [
            {
                "date": "2026-06-01",
                "avgtempC": "16",
                "avgtempF": "61",
                "mintempC": "13",
                "mintempF": "55",
                "maxtempC": "22",
                "maxtempF": "72",
                "hourly": [{
                    "time": "1200",
                    "weatherDesc": [{"value": "Overcast"}],
                    "chanceofrain": "5",
                    "chanceofsnow": "0",
                    "uvIndex": "3"
                }]
            },
            {
                "date": "2026-06-02",
                "avgtempC": "15",
                "avgtempF": "59",
                "mintempC": "13",
                "mintempF": "55",
                "maxtempC": "18",
                "maxtempF": "64",
                "hourly": [{
                    "time": "1200",
                    "weatherDesc": [{"value": "Light rain shower"}],
                    "chanceofrain": "80",
                    "chanceofsnow": "0",
                    "uvIndex": "2"
                }]
            }
        ]
    }"#;

    #[test]
    fn test_build_url_encodes_spaces() {
        let tool = GetWeatherTool::new();
        let url = tool.build_url("New York");
        assert_eq!(url, "https://wttr.in/New%20York?format=j1");
    }

    #[test]
    fn test_build_url_passthrough() {
        let tool = GetWeatherTool::new();
        let url = tool.build_url("London");
        assert_eq!(url, "https://wttr.in/London?format=j1");
    }

    #[test]
    fn test_parse_current_condition_valid() {
        let parsed = GetWeatherTool::parse_current_condition(MOCK_BODY).unwrap();
        assert_eq!(parsed["temperature_c"], "18");
        assert_eq!(parsed["feels_like_c"], "17");
        assert_eq!(parsed["description"], "Partly cloudy");
        assert_eq!(parsed["humidity_percent"], "65");
        assert_eq!(parsed["wind_kmph"], "12");
        assert_eq!(parsed["wind_direction"], "SW");
        assert_eq!(parsed["uv_index"], "4");
        assert_eq!(parsed["visibility_km"], "10");
        assert_eq!(parsed["pressure_mb"], "1015");
    }

    #[test]
    fn test_parse_current_condition_missing_fields() {
        let body = r#"{"current_condition": [{"temp_C": "20"}]}"#;
        let parsed = GetWeatherTool::parse_current_condition(body).unwrap();
        assert_eq!(parsed["temperature_c"], "20");
        assert_eq!(parsed["description"], "unknown");
    }

    #[test]
    fn test_parse_current_condition_invalid_json() {
        let result = GetWeatherTool::parse_current_condition("not json");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_current_condition_missing_current_condition() {
        let result = GetWeatherTool::parse_current_condition(r#"{"foo": "bar"}"#);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_forecast_valid() {
        let forecast = GetWeatherTool::parse_forecast(MOCK_BODY).unwrap();
        assert_eq!(forecast.len(), 2);

        assert_eq!(forecast[0]["date"], "2026-06-01");
        assert_eq!(forecast[0]["min_temp_c"], "13");
        assert_eq!(forecast[0]["max_temp_c"], "22");
        assert_eq!(forecast[0]["avg_temp_c"], "16");
        assert_eq!(forecast[0]["description"], "Overcast");
        assert_eq!(forecast[0]["chance_of_rain_percent"], "5");

        assert_eq!(forecast[1]["date"], "2026-06-02");
        assert_eq!(forecast[1]["description"], "Light rain shower");
        assert_eq!(forecast[1]["chance_of_rain_percent"], "80");
    }

    #[test]
    fn test_parse_forecast_missing_weather() {
        let result = GetWeatherTool::parse_forecast(r#"{"current_condition": [{}]}"#);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_forecast_day_fallback_hourly() {
        // No 1200 entry — should fall back to the first hourly entry.
        let body = r#"{"weather": [{
            "date": "2026-06-01",
            "mintempC": "10",
            "maxtempC": "20",
            "hourly": [{
                "time": "0",
                "weatherDesc": [{"value": "Clear"}],
                "chanceofrain": "0",
                "chanceofsnow": "0",
                "uvIndex": "1"
            }]
        }]}"#;
        let forecast = GetWeatherTool::parse_forecast(body).unwrap();
        assert_eq!(forecast[0]["description"], "Clear");
        assert_eq!(forecast[0]["chance_of_rain_percent"], "0");
    }
}
