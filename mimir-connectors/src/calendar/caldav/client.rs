//! CalDAV transport verbs (PROPFIND, sync-collection REPORT, PUT, DELETE).
//!
//! All request/response marshalling is HTTP-only; XML parsing lives in
//! [`super::xml`] and iCalendar decoding in [`super::ical`].

use std::time::Duration;

use crate::calendar::caldav::{
    CalDavAuth, CalDavClient, PutEventResult, SyncCollectionResult, parse_resourcetype_is_calendar,
    parse_sync_collection, propfind_method, report_method, xml_escape,
};
use crate::connector::ConnectorError;

impl CalDavClient {
    /// Build a client over the supplied HTTP client and credentials.
    pub fn new(http: reqwest::Client, auth: CalDavAuth) -> Self {
        Self { http, auth }
    }

    /// Build a client with a default HTTP backend (30 s timeout) and the given
    /// credentials.
    pub fn with_default_http(auth: CalDavAuth) -> Result<Self, ConnectorError> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| ConnectorError::Config(format!("HTTP client build failed: {e}")))?;
        Ok(Self::new(http, auth))
    }

    /// Apply the configured auth to a request builder.
    fn authed(&self, builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.auth {
            CalDavAuth::Basic { username, password } => {
                builder.basic_auth(username, Some(password))
            }
            CalDavAuth::Bearer { token } => builder.bearer_auth(token),
        }
    }

    /// PROPFIND (Depth 0) requesting `resourcetype`; returns whether the URL is
    /// a CalDAV calendar collection. Used for health probing.
    pub async fn is_calendar(&self, calendar_url: &str) -> Result<bool, ConnectorError> {
        let body = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
<d:propfind xmlns:d=\"DAV:\">\n  <d:prop>\n    <d:resourcetype/>\n  </d:prop>\n</d:propfind>";
        let resp = self
            .authed(self.http.request(propfind_method(), calendar_url))
            .header("Depth", "0")
            .header(
                reqwest::header::CONTENT_TYPE,
                "application/xml; charset=utf-8",
            )
            .body(body)
            .send()
            .await
            .map_err(|e| ConnectorError::Network(e.to_string()))?;
        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(ConnectorError::NotAuthenticated);
        }
        if !status.is_success() && status.as_u16() != 207 {
            return Err(ConnectorError::Other(format!(
                "PROPFIND failed: HTTP {status}"
            )));
        }
        let text = resp
            .text()
            .await
            .map_err(|e| ConnectorError::Network(e.to_string()))?;
        Ok(parse_resourcetype_is_calendar(&text))
    }

    /// sync-collection REPORT. `sync_token = None` performs a full sync and
    /// yields the initial token; `Some(token)` performs an incremental sync.
    pub async fn sync_collection(
        &self,
        calendar_url: &str,
        sync_token: Option<&str>,
    ) -> Result<SyncCollectionResult, ConnectorError> {
        let token_element = match sync_token {
            Some(t) => format!("<d:sync-token>{}</d:sync-token>", xml_escape(t)),
            None => "<d:sync-token/>".to_string(),
        };
        let body = format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
<d:sync-collection xmlns:d=\"DAV:\" xmlns:cal=\"urn:ietf:params:xml:ns:caldav\">\n  \
{token_element}\n  <d:sync-level>1</d:sync-level>\n  <d:prop>\n    <d:getetag/>\n    <cal:calendar-data/>\n  </d:prop>\n\
</d:sync-collection>"
        );
        let resp = self
            .authed(self.http.request(report_method(), calendar_url))
            .header("Depth", "1")
            .header(
                reqwest::header::CONTENT_TYPE,
                "application/xml; charset=utf-8",
            )
            .body(body)
            .send()
            .await
            .map_err(|e| ConnectorError::Network(e.to_string()))?;
        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(ConnectorError::NotAuthenticated);
        }
        // RFC 6578 §6.5: a truncated result set is signalled with HTTP 507
        // (Insufficient Storage) carrying a partial multistatus body plus an
        // advancing `sync-token`; accept it alongside 207 and parse the body
        // so the caller can page with the new token.
        if !status.is_success() && status.as_u16() != 207 && status.as_u16() != 507 {
            return Err(ConnectorError::Other(format!(
                "sync-collection REPORT failed: HTTP {status}"
            )));
        }
        let text = resp
            .text()
            .await
            .map_err(|e| ConnectorError::Network(e.to_string()))?;
        parse_sync_collection(&text)
    }

    /// Create or update a VEVENT via CalDAV `PUT` (RFC 4791 §5.5).
    ///
    /// `href` is the full resource URL to write; `ical` is the `VCALENDAR`
    /// text. `etag` is `Some` for an update (sent as `If-Match`) and `None`
    /// for a create (sent with `If-None-Match: *` so a stray overwrite of an
    /// existing resource fails 412 rather than clobbering it). Returns the
    /// new `ETag` when the server supplies one. A 401 maps to
    /// [`ConnectorError::NotAuthenticated`]; a 412 precondition failure
    /// surfaces as a distinct `Other` so the caller can retry with a fresh
    /// etag.
    pub async fn put_event(
        &self,
        href: &str,
        ical: &str,
        etag: Option<&str>,
    ) -> Result<PutEventResult, ConnectorError> {
        let mut builder = self
            .authed(self.http.request(reqwest::Method::PUT, href))
            .header(
                reqwest::header::CONTENT_TYPE,
                "text/calendar; charset=utf-8",
            )
            .body(ical.to_string());
        match etag {
            Some(tag) => builder = builder.header(reqwest::header::IF_MATCH, tag),
            None => builder = builder.header(reqwest::header::IF_NONE_MATCH, "*"),
        }
        let resp = builder
            .send()
            .await
            .map_err(|e| ConnectorError::Network(e.to_string()))?;
        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(ConnectorError::NotAuthenticated);
        }
        if status == reqwest::StatusCode::PRECONDITION_FAILED {
            return Err(ConnectorError::Other(format!(
                "PUT precondition failed (etag mismatch): HTTP {status}"
            )));
        }
        if !status.is_success() {
            return Err(ConnectorError::Other(format!(
                "PUT event failed: HTTP {status}"
            )));
        }
        let new_etag = resp
            .headers()
            .get(reqwest::header::ETAG)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        Ok(PutEventResult {
            href: href.to_string(),
            etag: new_etag,
        })
    }

    /// Delete a VEVENT via CalDAV `DELETE` (RFC 4791 §5.6).
    ///
    /// `etag` (when known) is sent as `If-Match`. Idempotent: a 404 is treated
    /// as success (the resource is already gone), so a duplicate delete after
    /// a server-side change never fails the caller.
    pub async fn delete_event(&self, href: &str, etag: Option<&str>) -> Result<(), ConnectorError> {
        let mut builder = self.authed(self.http.request(reqwest::Method::DELETE, href));
        if let Some(tag) = etag {
            builder = builder.header(reqwest::header::IF_MATCH, tag);
        }
        let resp = builder
            .send()
            .await
            .map_err(|e| ConnectorError::Network(e.to_string()))?;
        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(ConnectorError::NotAuthenticated);
        }
        if status == reqwest::StatusCode::PRECONDITION_FAILED {
            return Err(ConnectorError::Other(format!(
                "DELETE precondition failed (etag mismatch): HTTP {status}"
            )));
        }
        if !status.is_success() && status != reqwest::StatusCode::NOT_FOUND {
            return Err(ConnectorError::Other(format!(
                "DELETE event failed: HTTP {status}"
            )));
        }
        Ok(())
    }
}
