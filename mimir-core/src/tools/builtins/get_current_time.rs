use super::super::{Tool, ToolError, ToolOutput, ToolPermission};
use async_trait::async_trait;
use chrono::{DateTime, Local, TimeZone};
use serde_json::Value;

/// Returns the current localised time together with its UTC offset so the
/// agent can derive the actual UTC time. Falls back to UTC when the system
/// timezone cannot be resolved.
pub struct GetCurrentTimeTool;

/// Build a structured time payload from a timestamp, exposing the localised
/// RFC 3339 value, the equivalent UTC time, and the UTC offset string.
///
/// Generic over the timezone so it can be unit-tested with a fixed offset
/// without depending on the host's local timezone (issue #45).
pub fn local_time_payload<Tz: TimeZone>(now: DateTime<Tz>) -> Value
where
    <Tz as TimeZone>::Offset: std::fmt::Display,
{
    let utc = now.with_timezone(&chrono::Utc).to_rfc3339();
    serde_json::json!({
        "local": now.to_rfc3339(),
        "utc": utc,
        "offset": now.offset().to_string(),
    })
}

#[async_trait]
impl Tool for GetCurrentTimeTool {
    fn name(&self) -> &str {
        "get_current_time"
    }

    fn description(&self) -> &str {
        "Returns the current date and time in the user's localised time zone (RFC 3339), with the UTC offset and the equivalent UTC time so UTC can be derived."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {},
            "required": [],
            "additionalProperties": false,
        })
    }

    fn permission(&self) -> ToolPermission {
        ToolPermission::Auto
    }

    async fn execute(&self, _args: Value) -> Result<ToolOutput, ToolError> {
        let payload = local_time_payload(Local::now());
        Ok(ToolOutput {
            result: Some(payload),
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::local_time_payload;
    use chrono::{FixedOffset, TimeZone};

    fn dt(offset_secs: i32) -> chrono::DateTime<FixedOffset> {
        let tz = FixedOffset::east_opt(offset_secs).unwrap();
        tz.from_utc_datetime(
            &chrono::NaiveDate::from_ymd_opt(2026, 6, 23)
                .unwrap()
                .and_hms_opt(14, 30, 0)
                .unwrap(),
        )
    }

    #[test]
    fn local_time_payload_includes_offset_and_utc() {
        // British Summer Time: UTC+1.
        let payload = local_time_payload(dt(3600));
        assert_eq!(payload["offset"], "+01:00");
        assert!(payload["local"].as_str().unwrap().contains("+01:00"));
        assert_eq!(payload["utc"], "2026-06-23T14:30:00+00:00");
        assert!(payload["local"].as_str().unwrap().contains("15:30:00"));
    }

    #[test]
    fn local_time_payload_utc_has_zero_offset() {
        let payload = local_time_payload(dt(0));
        assert_eq!(payload["offset"], "+00:00");
        assert_eq!(payload["utc"], "2026-06-23T14:30:00+00:00");
        assert_eq!(payload["local"], "2026-06-23T14:30:00+00:00");
    }
}
