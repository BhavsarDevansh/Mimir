use crate::graph::KnowledgeGraph;
use crate::*;

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use mimir_core::geocoder::Geocoder;
use sqlx::SqlitePool;
use tokio::sync::{Mutex, Notify, RwLock, mpsc, oneshot};

use crate::clock::{Clock, RealClock};
use crate::graph::RelationshipTypeCache;
use crate::inference::RuleEngine;
use crate::inference::rules::contradiction::ContradictionRule;
use crate::inference::rules::threshold::ThresholdRule;
use crate::inference::rules::transitivity::TransitivityRule;
use crate::normalize::{OverlayJob, start_location_overlay_worker};

impl KnowledgeGraph {
    /// Initialise the knowledge graph: ensure parent directories exist, open
    /// the SQLite pool (WAL, foreign keys, bounded page cache, and bounded
    /// optimize-on-close), and run pending migrations.
    pub async fn init(db_path: &Path) -> Result<Self, KnowledgeError> {
        Self::init_with_clock(db_path, Arc::new(RealClock)).await
    }

    /// Initialise with a custom clock (used in tests for determinism).
    pub async fn init_with_clock(
        db_path: &Path,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, KnowledgeError> {
        if let Some(parent) = db_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let pool = db::create_pool(db_path).await?;
        sqlx::migrate!("src/db/migrations").run(&pool).await?;

        let mut engine = RuleEngine::new();
        engine.register(Box::new(TransitivityRule));
        engine.register(Box::new(ContradictionRule));
        engine.register(Box::new(ThresholdRule));

        let pending_ids: Vec<i32> =
            sqlx::query_scalar("SELECT id FROM facts WHERE pending_confirmation = TRUE")
                .fetch_all(&pool)
                .await?;

        let pending: HashSet<i32> = pending_ids.into_iter().collect();

        let write_lock = Arc::new(Mutex::new(()));
        let location_overlay_tx =
            start_location_overlay_worker(pool.clone(), Arc::clone(&write_lock));

        Ok(Self {
            pool,
            clock,
            relationship_type_cache: Arc::new(RwLock::new(RelationshipTypeCache::new())),
            centrality_cache: Arc::new(RwLock::new(HashMap::new())),
            rule_engine: engine,
            pending_confirmations: Arc::new(RwLock::new(pending)),
            condensation_dirty: AtomicBool::new(false),
            condensation_notify: Arc::new(Notify::new()),
            geocoder: None,
            location_overlay_tx,
            write_lock,
        })
    }

    /// Access the rule engine.
    pub(crate) fn rule_engine(&self) -> &RuleEngine {
        &self.rule_engine
    }

    /// Access the pending-confirmation in-memory cache.
    pub fn pending_confirmations(&self) -> &Arc<RwLock<HashSet<i32>>> {
        &self.pending_confirmations
    }

    /// Return the injected geocoder, if any (Phase 3 S3 / #193).
    pub fn geocoder(&self) -> Option<&Arc<dyn Geocoder>> {
        self.geocoder.as_ref()
    }

    /// Inject a geocoder backend for the entity-locations write path.
    ///
    /// Called once during server startup after the `KnowledgeGraph` is
    /// initialised, before connectors or the chat extraction path can produce
    /// location facts. Replaces any previously-injected backend.
    pub fn set_geocoder(&mut self, geocoder: Arc<dyn Geocoder>) {
        self.geocoder = Some(geocoder);
    }

    /// Sender for the location-overlay background worker (Phase 3 S3 / #193).
    pub(crate) fn location_overlay_tx(&self) -> &mpsc::UnboundedSender<OverlayJob> {
        &self.location_overlay_tx
    }

    /// Shared write-serialisation lock (issue #236). Holders perform all
    /// knowledge-graph *write* transactions under this mutex so the
    /// background overlay worker and ingestion callers never commit
    /// concurrently; see [`normalize_and_insert`] and
    /// [`start_location_overlay_worker`].
    pub(crate) fn write_lock(&self) -> &Arc<Mutex<()>> {
        &self.write_lock
    }

    /// Await every location-overlay job enqueued before this call.
    ///
    /// Location overlays are applied asynchronously by a background worker so
    /// the ingestion pipeline is not gated on the geocoder's rate limit. This
    /// barrier drains the worker's queue up to the call point: it enqueues a
    /// sentinel and resolves once the worker has finished every prior `Apply`
    /// job, so callers (graceful shutdown, tests) can read `entity_locations`
    /// deterministically. Jobs enqueued concurrently with the flush are not
    /// guaranteed to have completed.
    pub async fn flush_location_overlays(&self) {
        let (tx, rx) = oneshot::channel();
        if self.location_overlay_tx.send(OverlayJob::Flush(tx)).is_ok() {
            let _ = rx.await;
        }
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }
}
