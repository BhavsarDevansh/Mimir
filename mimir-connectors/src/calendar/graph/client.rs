//! Microsoft Graph transport: events delta query, JSON event decoding, and
//! the health probe.
//!
//! All request/response marshalling is HTTP + JSON. The delta query
//! (`GET /me/events/delta`) returns changed events plus an
//! `@odata.nextLink` (paging, `$skiptoken`) and a final `@odata.deltaLink`
//! (incremental cursor, `$deltatoken`); the connector persists the
//! deltaLink as its opaque cursor and re-requests it verbatim on the next
//! incremental cycle. Deleted events appear in the delta with an
//! `@removed` property and are reported as tombstones.
//!
//! # Security properties
//!
//! - **Bearer auth only** — the access token is attached per request; the
//!   client never follows redirects (the shared reqwest client is built
//!   with `redirect::Policy::none()`), so a compromised endpoint cannot
//!   bounce an authenticated request to another host.
//! - **Server-supplied links are origin-checked** — every `@odata.nextLink`
//!   / `@odata.deltaLink` must share the configured Graph service root's
//!   scheme, host, and port and lie under its path, so a malicious or
//!   compromised response cannot redirect the bearer token to an
//!   attacker-controlled host (mirrors the CalDAV `ensure_in_calendar`
//!   check).
//! - **401 maps to [`ConnectorError::NotAuthenticated`]** so the supervisor
//!   surfaces an expired/revoked token as `AuthExpired` and runs the
//!   one-shot forced-refresh retry (issue #507) instead of treating it as a
//!   generic failure.

use serde::Deserialize;

use crate::connector::ConnectorError;

/// Default Microsoft Graph service root (v1.0).
pub const GRAPH_BASE_URL: &str = "https://graph.microsoft.com/v1.0";

/// A Microsoft Graph event (the `$select`-limited delta shape).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct GraphEvent {
    /// Native event id — the `raw_reference` the extractor authors on every
    /// fact and the tombstone key for `@removed` events.
    pub id: String,
    /// Event title (`subject`).
    #[serde(default)]
    pub subject: Option<String>,
    /// Start instant (`start.dateTime` + `start.timeZone`).
    #[serde(default)]
    pub start: Option<GraphDateTime>,
    /// End instant (`end.dateTime` + `end.timeZone`).
    #[serde(default)]
    pub end: Option<GraphDateTime>,
    /// Venue (`location.displayName`).
    #[serde(default)]
    pub location: Option<GraphLocation>,
    /// Attendees (`attendees[].emailAddress`).
    #[serde(default)]
    pub attendees: Vec<GraphAttendee>,
    /// Recurrence (`recurrence.pattern.type`).
    #[serde(default)]
    pub recurrence: Option<GraphRecurrence>,
    /// Whether the event is cancelled (`isCancelled`). Retained for a
    /// future CANCEL lifecycle pass; fact extraction treats cancelled
    /// events like the CalDAV backend treats `STATUS:CANCELLED` (no special
    /// handling today).
    #[serde(default, rename = "isCancelled")]
    pub is_cancelled: bool,
    /// Delta deletion marker (`@removed`); `Some` means the event was
    /// removed from the calendar since the prior delta.
    #[serde(default, rename = "@removed")]
    pub removed: Option<GraphRemoved>,
}

/// A Graph `dateTime`/`timeZone` pair (ISO 8601 local time + IANA zone).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct GraphDateTime {
    /// ISO 8601 local datetime, e.g. `2025-05-03T09:00:00`.
    #[serde(rename = "dateTime")]
    pub date_time: String,
    /// IANA time zone the `dateTime` is expressed in (`UTC` by default).
    #[serde(rename = "timeZone")]
    pub time_zone: String,
}

/// A Graph `location` object.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct GraphLocation {
    /// Venue display name.
    #[serde(default, rename = "displayName")]
    pub display_name: Option<String>,
}

/// A Graph `attendees[]` entry.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct GraphAttendee {
    /// The attendee's email address + display name.
    #[serde(rename = "emailAddress")]
    pub email_address: GraphEmailAddress,
}

/// A Graph `emailAddress` object.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct GraphEmailAddress {
    /// Display name, when the address book knows one.
    #[serde(default)]
    pub name: Option<String>,
    /// SMTP address.
    #[serde(default)]
    pub address: Option<String>,
}

/// A Graph `recurrence` object.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct GraphRecurrence {
    /// The recurrence pattern (the `range` is not needed for fact
    /// extraction — the events subsystem advances recurring events).
    #[serde(default)]
    pub pattern: Option<GraphRecurrencePattern>,
}

/// A Graph `recurrence.pattern` object.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct GraphRecurrencePattern {
    /// Pattern type: `singleInstance`, `daily`, `weekly`,
    /// `absoluteMonthly`, `relativeMonthly`, `absoluteYearly`,
    /// `relativeYearly`.
    #[serde(rename = "type", default)]
    pub pattern_type: Option<String>,
}

/// A Graph `@removed` marker on a delta item.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct GraphRemoved {
    /// Removal reason (`deleted`, or `changed` when the item left the
    /// delta view but still exists).
    #[serde(default)]
    pub reason: Option<String>,
}

/// Outcome of one delta sync (possibly paged).
#[derive(Debug, Clone, PartialEq)]
pub struct GraphDeltaResult {
    /// Live (non-removed) events to stage for extraction.
    pub events: Vec<GraphEvent>,
    /// Event ids of `@removed` events — the tombstones the supervisor
    /// trashes via `extract_deletions` (issue #247).
    pub deleted: Vec<String>,
    /// The final `@odata.deltaLink` to persist as the incremental cursor.
    /// `None` when the server returned none (the connector clears its
    /// in-memory marker so the next cycle runs a full re-sync — the
    /// supervisor treats `None` as "cursor unchanged").
    pub new_delta_link: Option<String>,
}

/// One page of a delta response.
#[derive(Debug, Deserialize)]
struct GraphDeltaPage {
    #[serde(default)]
    value: Vec<GraphEvent>,
    #[serde(default, rename = "@odata.nextLink")]
    next_link: Option<String>,
    #[serde(default, rename = "@odata.deltaLink")]
    delta_link: Option<String>,
}

/// Microsoft Graph transport client.
pub struct GraphClient {
    http: reqwest::Client,
    base_url: String,
    token: String,
}

impl GraphClient {
    /// Build a client over the supplied HTTP client, service root, and
    /// bearer token.
    pub fn new(http: reqwest::Client, base_url: String, token: String) -> Self {
        Self {
            http,
            base_url,
            token,
        }
    }

    /// Apply the bearer token to a request builder.
    fn authed(&self, builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        builder.bearer_auth(&self.token)
    }

    /// Reject a server-supplied link (`@odata.nextLink` / `@odata.deltaLink`)
    /// that points outside the configured Graph service root, so a
    /// malicious or compromised response cannot redirect the bearer token
    /// to another host. The check is origin-aware: the scheme, host, and
    /// port must match the configured `base_url`, and the path must lie
    /// under the service root.
    fn ensure_same_origin(&self, link: &str) -> Result<(), ConnectorError> {
        let base = reqwest::Url::parse(self.base_url.trim_end_matches('/'))
            .map_err(|e| ConnectorError::Config(format!("invalid base_url: {e}")))?;
        let target = reqwest::Url::parse(link)
            .map_err(|e| ConnectorError::Config(format!("invalid Graph link `{link}`: {e}")))?;
        let same_origin = base.scheme() == target.scheme()
            && base.host_str() == target.host_str()
            && base.port() == target.port();
        if !same_origin {
            return Err(ConnectorError::Config(format!(
                "Graph link `{link}` is outside the configured service origin"
            )));
        }
        let base_path = base.path().trim_end_matches('/');
        let under = base_path.is_empty()
            || target.path() == base_path
            || target.path().starts_with(&format!("{base_path}/"));
        if !under {
            return Err(ConnectorError::Config(format!(
                "Graph link `{link}` is outside the configured service root"
            )));
        }
        Ok(())
    }

    /// Probe the service with the current token: a `$top=1` events read
    /// verifies both the credential and the `Calendars.Read` scope in one
    /// cheap round trip. Used by `authenticate` and `health`.
    pub async fn probe(&self) -> Result<(), ConnectorError> {
        let url = format!(
            "{}/me/events?$top=1&$select=id",
            self.base_url.trim_end_matches('/')
        );
        let resp = self
            .authed(self.http.get(&url))
            .send()
            .await
            .map_err(|e| ConnectorError::Network(e.to_string()))?;
        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(ConnectorError::NotAuthenticated);
        }
        if !status.is_success() {
            return Err(ConnectorError::Other(format!(
                "Microsoft Graph probe failed: HTTP {status}"
            )));
        }
        Ok(())
    }

    /// The initial (full-sync) delta query URL.
    fn delta_query_url(&self) -> String {
        format!(
            "{}/me/events/delta?$select=id,subject,start,end,location,attendees,recurrence,isCancelled",
            self.base_url.trim_end_matches('/')
        )
    }

    /// Whether a failed delta request signals a delta-token reset the
    /// client must recover from with a full synchronization (per the
    /// Microsoft Graph delta-query contract: a delta token can expire from
    /// the service's token cache or be invalidated by a server-side reset,
    /// in which case the service returns `410 Gone`, or a `400` whose body
    /// carries the `syncStateNotFound` error code, and the client must
    /// restart with a full sync).
    fn is_delta_reset(status: reqwest::StatusCode, body: &[u8]) -> bool {
        if status == reqwest::StatusCode::GONE {
            return true;
        }
        if status != reqwest::StatusCode::BAD_REQUEST {
            return false;
        }
        let Ok(payload) = serde_json::from_slice::<serde_json::Value>(body) else {
            return false;
        };
        payload.pointer("/error/code").and_then(|c| c.as_str()) == Some("syncStateNotFound")
    }

    /// Run one delta sync. `delta_link = None` performs a full sync and
    /// yields the initial deltaLink; `Some(link)` performs an incremental
    /// sync by requesting the stored deltaLink verbatim (it carries the
    /// `$deltatoken`). Pages through `@odata.nextLink` until the final
    /// `@odata.deltaLink` is reached. An expired or reset delta token (a
    /// `410 Gone`, or a `400 syncStateNotFound` response) restarts the sync
    /// from a full sync once — the Graph contract for a reset token — so a
    /// stale cursor self-heals instead of failing every cycle.
    pub async fn sync_events(
        &self,
        delta_link: Option<&str>,
    ) -> Result<GraphDeltaResult, ConnectorError> {
        let mut events = Vec::new();
        let mut deleted = Vec::new();
        let mut url = match delta_link {
            Some(link) => {
                self.ensure_same_origin(link)?;
                link.to_string()
            }
            None => self.delta_query_url(),
        };
        // A delta token can expire between cycles (the service's delta-token
        // cache evicts old tokens) or be invalidated by a server-side reset;
        // the Graph contract then answers `410 Gone` (or `400` with
        // `syncStateNotFound`) and the client must restart with a full
        // synchronization. Exactly one restart: a full sync cannot hit the
        // reset path again, so any further reset is a genuine server error.
        let mut restarting = delta_link.is_some();
        let new_delta_link = loop {
            let resp = self
                .authed(self.http.get(&url))
                // Ask for UTC datetimes so `start.timeZone`/`end.timeZone`
                // are deterministic (`UTC`) regardless of the mailbox's
                // display zone.
                .header("Prefer", "outlook.timezone=\"UTC\"")
                .send()
                .await
                .map_err(|e| ConnectorError::Network(e.to_string()))?;
            let status = resp.status();
            if status == reqwest::StatusCode::UNAUTHORIZED {
                return Err(ConnectorError::NotAuthenticated);
            }
            if !status.is_success() {
                // Only the reset-check statuses need the response body; any
                // other failure returns without buffering a (potentially
                // large) error payload.
                if restarting
                    && (status == reqwest::StatusCode::GONE
                        || status == reqwest::StatusCode::BAD_REQUEST)
                {
                    let body = resp
                        .bytes()
                        .await
                        .map_err(|e| ConnectorError::Network(e.to_string()))?;
                    if Self::is_delta_reset(status, &body) {
                        // Restart from a full sync: re-fetch everything and
                        // discard the partial page (if any) collected so far.
                        events.clear();
                        deleted.clear();
                        url = self.delta_query_url();
                        restarting = false;
                        continue;
                    }
                }
                return Err(ConnectorError::Other(format!(
                    "Microsoft Graph events delta failed: HTTP {status}"
                )));
            }
            let body = resp
                .bytes()
                .await
                .map_err(|e| ConnectorError::Network(e.to_string()))?;
            let page: GraphDeltaPage = serde_json::from_slice(&body).map_err(|e| {
                ConnectorError::Parse(format!("invalid Microsoft Graph events response: {e}"))
            })?;
            for event in page.value {
                if event.removed.is_some() {
                    deleted.push(event.id.clone());
                } else {
                    events.push(event);
                }
            }
            if let Some(next) = page.next_link {
                self.ensure_same_origin(&next)?;
                url = next;
                continue;
            }
            break page.delta_link;
        };
        Ok(GraphDeltaResult {
            events,
            deleted,
            new_delta_link,
        })
    }
}
