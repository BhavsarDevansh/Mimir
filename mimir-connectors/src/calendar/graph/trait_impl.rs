//! [`Connector`] trait implementation for the Microsoft Graph calendar
//! backend.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;

use crate::calendar::graph::GraphCalendarConnector;
use crate::calendar::graph::client::{GraphClient, GraphEvent};
use crate::connector::{
    Connector, ConnectorError, ConnectorMode, CredentialRefresh, HealthStatus, SyncOptions,
    SyncOutcome,
};
use crate::secrets::SecretBundle;
use mimir_knowledge::models::enums::{ConnectorAuthState, ConnectorType};
use mimir_knowledge::normalize::NormalizedFact;

#[async_trait]
impl CredentialRefresh for GraphCalendarConnector {
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
impl Connector for GraphCalendarConnector {
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
            "required": ["auth"],
            "properties": {
                "auth": {
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
                },
                "base_url": { "type": "string", "format": "uri" },
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
        let (token, refreshed) = self.resolve_auth(&bundle, false).await?;
        if let Some(refreshed) = refreshed {
            self.persist_refreshed(&refreshed).await?;
        }
        // Probe the service with the resolved token (a `$top=1` events read
        // verifies both the credential and the `Calendars.Read` scope).
        match GraphClient::new(self.http.clone(), self.base_url().to_string(), token)
            .probe()
            .await
        {
            Ok(()) => Ok(ConnectorAuthState::Authenticated),
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
        match client.probe().await {
            Ok(()) => Ok(HealthStatus::Online),
            Err(ConnectorError::NotAuthenticated) => Ok(HealthStatus::AuthExpired(
                "Microsoft Graph rejected the credentials (HTTP 401)".to_string(),
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
        let delta_link = if options.full {
            None
        } else {
            self.delta_link.lock().await.clone()
        };
        let result = client.sync_events(delta_link.as_deref()).await?;
        // The cursor the supervisor persists is the delta link the server
        // returned for this cycle; the in-memory marker only adopts it via
        // `on_cycle_succeeded` once the whole cycle succeeded (issue #314).
        let new_cursor = result.new_delta_link.clone();
        let fetched = self.stage(result).await?;
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
        if let Some(link) = new_cursor {
            *self.delta_link.lock().await = Some(link.to_string());
        }
    }

    async fn extract(&self) -> Result<Vec<NormalizedFact>, ConnectorError> {
        // Drain the staged Graph events and convert each into a small
        // cluster of `NormalizedFact`s. The shared `normalize_and_insert`
        // pipeline (run by the supervisor) resolves every subject/object
        // entity via the full F5 chain, assigns connector confidence, and —
        // for the primary `user has_event <event>` fact — derives the
        // events-subsystem (#74) overlay so future-dated and recurring
        // events surface in the user's "Upcoming" memory section.
        let mut buffer = self.buffer.lock().await;
        let staged: Vec<GraphEvent> = std::mem::take(&mut *buffer);
        let mut facts = Vec::new();
        for event in &staged {
            facts.extend(self.event_to_facts(event));
        }
        Ok(facts)
    }

    async fn extract_deletions(&self) -> Result<Vec<String>, ConnectorError> {
        // Issue #247: report the staged tombstones (the event ids the server
        // reported `@removed` in `sync`) without draining them — the
        // supervisor acknowledges the processed removals via
        // `acknowledge_deletions` only after trashing, fact insertion, and
        // cursor persistence all succeeded, so a failed cycle re-reports
        // them instead of losing them (PR #313 review). Each id is the
        // `raw_reference` the extractor authored for the deleted event's
        // facts, so the supervisor trashes exactly those facts.
        let tombstones = self.tombstones.lock().await;
        Ok(tombstones.clone())
    }

    async fn acknowledge_deletions(&self, deleted: &[String]) -> Result<(), ConnectorError> {
        let mut tombstones = self.tombstones.lock().await;
        tombstones.retain(|id| !deleted.contains(id));
        Ok(())
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
