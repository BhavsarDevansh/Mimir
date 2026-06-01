use super::super::{Tool, ToolError, ToolOutput, ToolPermission};
use async_trait::async_trait;
use serde_json::Value;

const DEFAULT_BASE_URL: &str = "https://wttr.in";

/// Fetches current weather conditions for a given location using wttr.in.
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
    /// Spaces are percent-encoded to `%20`.
    fn build_url(&self, location: &str) -> String {
        let encoded = location.replace(' ', "%20");
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

    /// Extract a concise weather summary from the wttr.in JSON response.
    fn parse_response(body: &str) -> Result<Value, ToolError> {
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
            "temperature_f": Self::extract_str(current, "temp_F"),
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
}

#[async_trait]
impl Tool for GetWeatherTool {
    fn name(&self) -> &str {
        "get_weather"
    }

    fn description(&self) -> &str {
        "Fetches current weather conditions for a given location using wttr.in. \
Returns temperature, conditions, humidity, wind, UV index, visibility, and pressure."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "location": {
                    "type": "string",
                    "description": "The location to get weather for. Can be a city name, airport code, or coordinates."
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

        let result = Self::parse_response(&body)?;

        Ok(ToolOutput {
            result: Some(result),
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn test_parse_response_valid() {
        let body = r#"{
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
            }]
        }"#;

        let parsed = GetWeatherTool::parse_response(body).unwrap();
        assert_eq!(parsed["temperature_c"], "18");
        assert_eq!(parsed["temperature_f"], "64");
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
    fn test_parse_response_missing_fields() {
        let body = r#"{"current_condition": [{"temp_C": "20"}]}"#;
        let parsed = GetWeatherTool::parse_response(body).unwrap();
        assert_eq!(parsed["temperature_c"], "20");
        assert_eq!(parsed["description"], "unknown");
    }

    #[test]
    fn test_parse_response_invalid_json() {
        let result = GetWeatherTool::parse_response("not json");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_response_missing_current_condition() {
        let result = GetWeatherTool::parse_response(r#"{"foo": "bar"}"#);
        assert!(result.is_err());
    }
}
