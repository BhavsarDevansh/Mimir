//! [`Connector`] trait implementation for the CalDAV backend.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;

use crate::calendar::CalendarConnector;
use crate::calendar::caldav::{CalDavClient, RawCalDavEvent};
use crate::calendar::payload::{DeleteEventPayload, WriteEventPayload, build_vevent};
use crate::connector::{
    ActionResult, Connector, ConnectorAction, ConnectorError, ConnectorMode, CredentialRefresh,
    HealthStatus, SyncOptions, SyncOutcome,
};
use crate::secrets::SecretBundle;
use mimir_knowledge::models::enums::{ConnectorAuthState, ConnectorType};
use mimir_knowledge::normalize::NormalizedFact;

#[async_trait]
impl CredentialRefresh for CalendarConnector {
    fn secret_store(&self) -> Option<Arc<dyn crate::secrets::SecretStore>> {
        self.secret_store.clone()
    }

    fn connector_slug(&self) -> &str {
        &self.slug
    }

    async fn forced_refresh(
        &self,
        bundle: &SecretBundle,
    ) -> Result<Option<SecretBundle>, ConnectorError> {
        self.resolve_auth(bundle, true)
            .await
            .map(|(_, refreshed)| refreshed)
    }

    async fn persist_refreshed_bundle(&self, bundle: &SecretBundle) -> Result<(), ConnectorError> {
        self.persist_refreshed(bundle).await
    }
}

#[async_trait]
impl Connector for CalendarConnector {
    fn id(&self) -> &str {
        &self.slug
    }

    fn name(&self) -> &str {
        &self.display_name
    }

    fn connector_type(&self) -> ConnectorType {
        ConnectorType::Calendar
    }

    fn mode(&self) -> ConnectorMode {
        ConnectorMode::Polling {
            interval: Duration::from_secs(self.config.poll_interval_secs),
            jitter: Duration::from_secs(self.config.poll_jitter_secs),
        }
    }

    fn config_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["calendar_url", "auth"],
            "properties": {
                "calendar_url": { "type": "string", "format": "uri" },
                "auth": {
                    "oneOf": [
                        {
                            "type": "object",
                            "required": ["kind", "username"],
                            "properties": {
                                "kind": { "const": "app_password" },
                                "username": { "type": "string" }
                            }
                        },
                        {
                            "type": "object",
                            "required": ["kind", "auth_uri", "token_endpoint", "client_id"],
                            "properties": {
                                "kind": { "const": "oauth" },
                                "auth_uri": { "type": "string", "format": "uri" },
                                "token_endpoint": { "type": "string", "format": "uri" },
                                "client_id": { "type": "string" },
                                "client_secret": { "type": "string" },
                                "scopes": { "type": "array", "items": { "type": "string" } }
                            }
                        }
                    ]
                },
                "poll_interval_secs": { "type": "integer", "default": 900 },
                "poll_jitter_secs": { "type": "integer", "default": 60 },
                "display_name": { "type": "string" }
            }
        })
    }

    async fn authenticate(&self) -> Result<ConnectorAuthState, ConnectorError> {
        let store = self
            .secret_store
            .clone()
            .ok_or(ConnectorError::NotAuthenticated)?;
        let bundle = store
            .load(&self.slug)
            .await
            .map_err(|e| ConnectorError::Authentication(format!("secret load failed: {e}")))?;
        let Some(bundle) = bundle else {
            return Ok(ConnectorAuthState::Unauthenticated);
        };
        let (auth, refreshed) = self.resolve_auth(&bundle, false).await?;
        if let Some(refreshed) = refreshed {
            self.persist_refreshed(&refreshed).await?;
        }
        // Probe the server with the resolved credentials (PROPFIND resourcetype).
        match CalDavClient::new(self.http.clone(), auth)
            .is_calendar(&self.config.calendar_url)
            .await
        {
            Ok(true) => Ok(ConnectorAuthState::Authenticated),
            Ok(false) => Err(ConnectorError::Config(format!(
                "configured URL is not a CalDAV calendar: {}",
                self.config.calendar_url
            ))),
            Err(ConnectorError::NotAuthenticated) => Ok(ConnectorAuthState::Expired),
            Err(e) => Err(e),
        }
    }

    async fn health(&self) -> Result<HealthStatus, ConnectorError> {
        let (client, refreshed) = match self.client_from_credentials().await {
            Ok(pair) => pair,
            Err(ConnectorError::NotAuthenticated) => return Ok(HealthStatus::NotConfigured),
            Err(ConnectorError::Authentication(message)) => {
                return Ok(HealthStatus::AuthExpired(message));
            }
            Err(e) => return Err(e),
        };
        if let Some(b) = refreshed {
            self.persist_refreshed(&b).await?;
        }
        match client.is_calendar(&self.config.calendar_url).await {
            Ok(true) => Ok(HealthStatus::Online),
            Ok(false) => Ok(HealthStatus::Degraded),
            Err(ConnectorError::NotAuthenticated) => Ok(HealthStatus::AuthExpired(
                "CalDAV server rejected the credentials (HTTP 401)".to_string(),
            )),
            Err(ConnectorError::Network(_)) => Ok(HealthStatus::Offline),
            Err(e) => Err(e),
        }
    }

    async fn force_refresh(&self) -> Result<ConnectorAuthState, ConnectorError> {
        self.force_refresh_credentials().await
    }

    async fn sync(&self, options: SyncOptions) -> Result<SyncOutcome, ConnectorError> {
        let (client, refreshed) = self.client_from_credentials().await?;
        if let Some(b) = refreshed {
            self.persist_refreshed(&b).await?;
        }
        // Full sync ignores the persisted cursor (re-fetch everything).
        let mut token = if options.full {
            None
        } else {
            self.sync_token.lock().await.clone()
        };
        let mut fetched = 0u32;
        let mut new_cursor;
        loop {
            let result = client
                .sync_collection(&self.config.calendar_url, token.as_deref())
                .await?;
            let truncated = result.truncated;
            new_cursor = result.new_sync_token.clone();
            fetched = fetched.saturating_add(self.stage(result).await?);
            // RFC 6578 §6.5: a truncated (507) response returns a partial set
            // plus an advancing sync-token — re-request with it to page
            // through the remaining changes until the collection is drained.
            if !truncated {
                break;
            }
            token = new_cursor.clone();
            if token.is_none() {
                // No advancing token: cannot make progress, stop paging.
                break;
            }
        }
        Ok(SyncOutcome {
            fetched,
            new_cursor,
            fetched_at: Utc::now(),
        })
    }

    async fn on_cycle_succeeded(&self, new_cursor: Option<&str>) {
        // Adopt the persisted cursor as the in-memory progress marker only
        // now that the supervisor confirmed the whole cycle succeeded (issue
        // #314). Advancing inside `sync` would skip the failed cycle's
        // changed events on the next in-process cycle: the persisted cursor
        // is only updated on a fully successful cycle, so the in-memory
        // marker must never run ahead of it. `None` means "cursor unchanged"
        // and leaves the marker as-is.
        if let Some(tok) = new_cursor {
            *self.sync_token.lock().await = Some(tok.to_string());
        }
    }

    async fn extract(&self) -> Result<Vec<NormalizedFact>, ConnectorError> {
        // C4 / #198: drain the staged VEVENTs and convert each into a small
        // cluster of `NormalizedFact`s. The shared `normalize_and_insert`
        // pipeline (run by the supervisor) resolves every subject/object
        // entity via the full F5 chain, assigns connector confidence, and —
        // for the primary `user has_event <event>` fact — derives the
        // events-subsystem (#74) overlay so future-dated and recurring
        // events surface in the user's "Upcoming" memory section.
        let mut buffer = self.buffer.lock().await;
        let staged: Vec<RawCalDavEvent> = std::mem::take(&mut *buffer);
        let mut facts = Vec::new();
        for event in &staged {
            facts.extend(self.event_to_facts(event));
        }
        Ok(facts)
    }

    async fn extract_deletions(&self) -> Result<Vec<String>, ConnectorError> {
        // Issue #247: report the staged tombstones (the hrefs the server
        // reported deleted in `sync`) without draining them — the supervisor
        // acknowledges the processed removals via `acknowledge_deletions`
        // only after trashing, fact insertion, and cursor persistence all
        // succeeded, so a failed cycle re-reports them instead of losing
        // them (PR #313 review). Each href is the `raw_reference` the
        // extractor authored for the deleted event's facts, so the supervisor
        // trashes exactly those facts.
        let tombstones = self.tombstones.lock().await;
        Ok(tombstones.clone())
    }

    async fn acknowledge_deletions(&self, deleted: &[String]) -> Result<(), ConnectorError> {
        let mut tombstones = self.tombstones.lock().await;
        tombstones.retain(|href| !deleted.contains(href));
        Ok(())
    }

    /// CalDAV write-back (C4 / #198): the only connector with write support.
    ///
    /// Three action kinds, each authenticated via the same credential path
    /// as `sync` (OAuth refresh included):
    /// - `create_event` — builds a VEVENT from the payload, generates a `UID`
    ///   (unless supplied), and `PUT`s it to `<calendar>/<uid>.ics` with
    ///   `If-None-Match: *`.
    /// - `update_event` — requires the target `href` (and optional `etag`),
    ///   `PUT`s with `If-Match: <etag>`.
    /// - `delete_event` — requires the target `href` (and optional `etag`),
    ///   `DELETE`s it (idempotent on 404).
    async fn act(&self, action: ConnectorAction) -> Result<ActionResult, ConnectorError> {
        let (client, refreshed) = self.client_from_credentials().await?;
        if let Some(b) = refreshed {
            self.persist_refreshed(&b).await?;
        }
        match action.kind.as_str() {
            "create_event" => {
                let p: WriteEventPayload = serde_json::from_value(action.payload)
                    .map_err(|e| ConnectorError::Config(format!("create_event payload: {e}")))?;
                let uid = p
                    .uid
                    .clone()
                    .unwrap_or_else(|| format!("{}", uuid::Uuid::new_v4()));
                let href = p.href.clone().unwrap_or_else(|| {
                    format!(
                        "{}/{uid}.ics",
                        self.config.calendar_url.trim_end_matches('/')
                    )
                });
                self.ensure_in_calendar(&href)?;
                let ical = build_vevent(&p, &uid)?;
                let res = client.put_event(&href, &ical, None).await?;
                Ok(ActionResult {
                    success: true,
                    native_id: Some(res.href),
                    message: res.etag,
                })
            }
            "update_event" => {
                let p: WriteEventPayload = serde_json::from_value(action.payload)
                    .map_err(|e| ConnectorError::Config(format!("update_event payload: {e}")))?;
                let href = p
                    .href
                    .clone()
                    .ok_or_else(|| ConnectorError::Config("update_event requires `href`".into()))?;
                self.ensure_in_calendar(&href)?;
                let uid = p.uid.clone().unwrap_or_else(|| {
                    href.rsplit('/')
                        .next()
                        .map(|s| s.trim_end_matches(".ics").to_string())
                        .unwrap_or_default()
                });
                let ical = build_vevent(&p, &uid)?;
                let res = client.put_event(&href, &ical, p.etag.as_deref()).await?;
                Ok(ActionResult {
                    success: true,
                    native_id: Some(res.href),
                    message: res.etag,
                })
            }
            "delete_event" => {
                let p: DeleteEventPayload = serde_json::from_value(action.payload)
                    .map_err(|e| ConnectorError::Config(format!("delete_event payload: {e}")))?;
                let href = p.href.clone();
                self.ensure_in_calendar(&href)?;
                client.delete_event(&href, p.etag.as_deref()).await?;
                Ok(ActionResult {
                    success: true,
                    native_id: Some(href),
                    message: None,
                })
            }
            other => Err(ConnectorError::UnsupportedAction(other.to_string())),
        }
    }

    async fn forget(&self) -> Result<(), ConnectorError> {
        self.buffer.lock().await.clear();
        if let Some(store) = &self.secret_store {
            store.delete(&self.slug).await.map_err(|e| {
                ConnectorError::Authentication(format!("secret delete failed: {e}"))
            })?;
        }
        Ok(())
    }
}
