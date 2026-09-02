use crate::graph::KnowledgeGraph;
use crate::*;

impl KnowledgeGraph {
    // ------------------------------------------------------------------
    // Connector reliability
    // ------------------------------------------------------------------

    /// Adjust a connector's reliability score.
    pub async fn adjust_connector_reliability(
        &self,
        connector: models::enums::ConnectorType,
        delta: f32,
    ) -> Result<(), KnowledgeError> {
        confidence::adjust_connector_reliability(&self.pool, connector, delta).await
    }

    /// Read a connector's current reliability score.
    pub async fn connector_reliability(
        &self,
        connector: models::enums::ConnectorType,
    ) -> Result<f32, KnowledgeError> {
        confidence::connector_reliability(&self.pool, connector).await
    }

    // ------------------------------------------------------------------
    // Connector instance registry delegates (issue #179 / Phase 3 F2)
    // ------------------------------------------------------------------

    /// List every registered connector instance, oldest first.
    pub async fn list_connectors(
        &self,
    ) -> Result<Vec<models::connector::Connector>, KnowledgeError> {
        queries::connector::list_connectors(&self.pool).await
    }

    /// Fetch a connector instance by its unique human label (`slug`).
    pub async fn get_connector_by_slug(
        &self,
        slug: &str,
    ) -> Result<Option<models::connector::Connector>, KnowledgeError> {
        queries::connector::get_connector_by_slug(&self.pool, slug).await
    }

    /// Fetch a connector instance by its integer primary key.
    pub async fn get_connector(
        &self,
        id: i32,
    ) -> Result<Option<models::connector::Connector>, KnowledgeError> {
        queries::connector::get_connector(&self.pool, id).await
    }

    /// Insert a new connector instance or update the mutable config surface of
    /// an existing one (keyed on `slug`). Sync-progress fields are preserved on
    /// conflict.
    pub async fn upsert_connector(
        &self,
        input: models::connector::UpsertConnectorInput,
    ) -> Result<models::connector::Connector, KnowledgeError> {
        queries::connector::upsert_connector(&self.pool, &input, self.now()).await
    }

    /// Atomically insert a **new** connector instance, relying on the
    /// `connectors.slug UNIQUE` index to reject a duplicate slug with
    /// [`KnowledgeError::ConnectorSlugConflict`] (so two concurrent creates
    /// for the same slug cannot both succeed). Use this for the add-only
    /// `POST /connectors` route; reconfiguring an existing instance is A2 /
    /// #203 and uses [`Self::upsert_connector`].
    pub async fn create_connector(
        &self,
        input: models::connector::UpsertConnectorInput,
    ) -> Result<models::connector::Connector, KnowledgeError> {
        queries::connector::create_connector(&self.pool, &input, self.now()).await
    }

    /// Advance a connector's opaque sync cursor, stamping `last_sync_at`.
    /// `cursor = None` clears the cursor (e.g. for a full re-sync).
    pub async fn update_sync_cursor(
        &self,
        id: i32,
        cursor: Option<&str>,
    ) -> Result<models::connector::Connector, KnowledgeError> {
        queries::connector::update_sync_cursor(&self.pool, id, cursor, self.now()).await
    }

    /// Persist a connector's opaque durable state (e.g. the Email connector's
    /// LLM-extraction retry ledger, issue #262). The supervisor calls this
    /// after each successful extraction cycle with the value returned by the
    /// connector's `durable_state()` hook (from `mimir-connectors`) and
    /// re-injects it at construction as `__durable_state`, so retries and
    /// terminal failures survive daemon restarts. `state = None` clears it.
    pub async fn update_durable_state(
        &self,
        id: i32,
        state: Option<&str>,
    ) -> Result<models::connector::Connector, KnowledgeError> {
        queries::connector::update_durable_state(&self.pool, id, state, self.now()).await
    }

    /// Advance the sync cursor (or stamp `last_sync_at` when the cursor is
    /// unchanged) and persist the connector's durable state in one
    /// transaction, so a crash between the two writes cannot advance the
    /// cursor without its durable state (issue #262 / PR #318 review). The
    /// supervisor uses this instead of [`Self::update_sync_cursor`] /
    /// [`Self::touch_last_sync`] followed by [`Self::update_durable_state`]:
    /// the cursor and the retry ledger must commit together, or a restart
    /// would skip a failed message whose retry record was lost.
    ///
    /// `cursor = None` means "unchanged" (the `SyncOutcome::new_cursor`
    /// semantics — unlike [`Self::update_sync_cursor`], where `None`
    /// clears); `durable_state = None` means "unchanged" too.
    pub async fn update_sync_progress_and_durable_state(
        &self,
        id: i32,
        cursor: Option<&str>,
        durable_state: Option<&str>,
    ) -> Result<models::connector::Connector, KnowledgeError> {
        queries::connector::update_sync_progress_and_durable_state(
            &self.pool,
            id,
            cursor,
            durable_state,
            self.now(),
        )
        .await
    }

    /// Stamp `last_sync_at` **without** rewriting `sync_cursor`.
    ///
    /// Use this when a connector reports `SyncOutcome::new_cursor = None`
    /// (meaning "cursor unchanged") so the persisted progress token is
    /// preserved while the sync timestamp is still advanced.
    pub async fn touch_last_sync(
        &self,
        id: i32,
    ) -> Result<models::connector::Connector, KnowledgeError> {
        queries::connector::touch_last_sync(&self.pool, id, self.now()).await
    }

    /// Transition a connector to a new lifecycle status, optionally touching
    /// `last_error`. See [`queries::connector::set_connector_status`] for the
    /// `error` nullable-update semantics.
    pub async fn set_connector_status(
        &self,
        id: i32,
        status: models::enums::ConnectorStatus,
        error: Option<Option<String>>,
    ) -> Result<models::connector::Connector, KnowledgeError> {
        queries::connector::set_connector_status(
            &self.pool,
            id,
            status,
            error.as_ref().map(|o| o.as_deref()),
            self.now(),
        )
        .await
    }

    /// Set a connector's auth state.
    pub async fn set_auth_state(
        &self,
        id: i32,
        auth_state: models::enums::ConnectorAuthState,
    ) -> Result<models::connector::Connector, KnowledgeError> {
        queries::connector::set_auth_state(&self.pool, id, auth_state, self.now()).await
    }

    /// Replace a connector's persisted `config_json` (the token-ingest route
    /// uses this to persist the non-secret OAuth config alongside a
    /// re-authed credential bundle, issue #507 review). Sync-progress
    /// columns are untouched; only `config_json` and `updated_at` change.
    pub async fn update_connector_config(
        &self,
        id: i32,
        config_json: &str,
    ) -> Result<models::connector::Connector, KnowledgeError> {
        queries::connector::update_connector_config(&self.pool, id, config_json, self.now()).await
    }

    /// Increment a connector's cumulative fact-acceptance counters (issue
    /// #508): `accepted` validated LLM facts vs `dropped` LLM facts rejected
    /// by Rust-side validation. Called by the email prose-extraction hook so
    /// `mimir connector list` / `status` can surface the acceptance rate.
    pub async fn record_connector_fact_counts(
        &self,
        id: i32,
        accepted: i64,
        dropped: i64,
        staged: i64,
    ) -> Result<(), KnowledgeError> {
        queries::connector::record_connector_fact_counts(
            &self.pool,
            id,
            accepted,
            dropped,
            staged,
            self.now(),
        )
        .await
    }

    /// Number of `sources` rows attributed to a connector instance — the
    /// derived "items ingested" metric for the connector status endpoint
    /// (issue #202 / Phase 3 A1).
    pub async fn count_sources_for_connector(&self, id: i32) -> Result<i64, KnowledgeError> {
        queries::connector::count_sources_for_connector(&self.pool, id).await
    }

    /// Item counts for every connector instance in one query — a map of
    /// `connector_id -> items ingested` (instances with no facts are absent;
    /// treat a missing key as `0`). Used by the list route so item counts are
    /// derived in one round-trip rather than N+1 (issue #202 / A1).
    pub async fn count_sources_by_connector(
        &self,
    ) -> Result<std::collections::HashMap<i32, i64>, KnowledgeError> {
        queries::connector::count_sources_by_connector(&self.pool).await
    }

    /// Delete a connector instance, detaching its provenance first. See
    /// [`queries::connector::delete_connector`] for the SET NULL FK
    /// semantics; the full `forget` cascade is deferred to A2 / #203.
    pub async fn delete_connector(&self, id: i32) -> Result<(), KnowledgeError> {
        queries::connector::delete_connector(&self.pool, id).await
    }
}
