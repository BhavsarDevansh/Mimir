use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use chrono::Utc;
use dashmap::DashMap;

use mimir_connectors::{ConnectorRegistry, ConnectorSupervisor, SupervisorConfig};
use mimir_core::{
    agents::AgentRuntime,
    config::{Config, ReloadableConfig},
    context::ContextManager,
    geocoder::Geocoder,
    job_queue::{Job, JobContext, JobPriority, JobQueue},
    llm::{LlmBackend, LlmClient},
    scheduler::{BackgroundScheduler, DaemonJob},
    tools::ToolRegistry,
};

use super::identity::seed_identity_facts;
use super::{AppState, warn_err};

fn register_tool(registry: &ToolRegistry, tool: Arc<dyn mimir_core::tools::Tool>) {
    let name = tool.name().to_string();
    warn_err(
        registry.register_native(tool),
        &format!("Failed to register {name} tool"),
    );
}

/// Outputs of [`init_knowledge_graph`] consumed by later init steps.
pub(super) struct KnowledgeGraphInit {
    pub(super) knowledge_graph: Arc<mimir_knowledge::KnowledgeGraph>,
    /// Shared geocoder handed to the connector supervisor (C2 / #196).
    pub(super) geocoder: Option<Arc<dyn Geocoder>>,
    /// Cached user entity ID (resolved or created at startup).
    pub(super) user_entity_id: Option<i32>,
    /// Directory for knowledge-graph backups, alongside the (possibly
    /// overridden) knowledge DB (issue #233).
    pub(super) backup_dir: std::path::PathBuf,
}

/// Initialise the conversation context manager from the configured DB path.
pub(super) async fn init_context_manager(cfg: &Config) -> anyhow::Result<Arc<ContextManager>> {
    let db_path = mimir_core::paths::resolve_db_path(
        cfg.context.db_path.clone(),
        mimir_core::paths::default_db_path,
    )?;
    Ok(Arc::new(ContextManager::new(&db_path).await?))
}

/// Register the built-in tools and load the user tools config, if present.
pub(super) fn init_tool_registry(context_manager: &Arc<ContextManager>) -> Arc<ToolRegistry> {
    let tool_registry = Arc::new(ToolRegistry::new());
    register_tool(
        &tool_registry,
        Arc::new(mimir_core::tools::GetCurrentTimeTool),
    );
    register_tool(&tool_registry, Arc::new(mimir_core::tools::EchoTool));
    register_tool(
        &tool_registry,
        Arc::new(mimir_core::tools::GetWeatherTool::new()),
    );
    register_tool(
        &tool_registry,
        Arc::new(mimir_core::tools::SearchConversationHistoryTool::new(
            Arc::clone(context_manager),
        )),
    );
    if let Some(path) = mimir_core::tools::ToolsConfig::default_path()
        && path.exists()
    {
        warn_err(
            tool_registry.load_tools_config(&path),
            "Failed to load tools config",
        );
    }
    tool_registry
}

/// Initialise the knowledge graph: open the DB, inject the default geocoder,
/// resolve (or create) the user entity, seed identity facts, and register the
/// knowledge-graph tools.
pub(super) async fn init_knowledge_graph(
    cfg: &Config,
    tool_registry: &ToolRegistry,
    context_manager: &Arc<ContextManager>,
    llm: &Arc<dyn LlmBackend>,
) -> anyhow::Result<KnowledgeGraphInit> {
    let kg_db_path = mimir_core::paths::resolve_db_path(
        cfg.knowledge.db_path.clone(),
        mimir_core::paths::knowledge_db_path,
    )?;
    // Knowledge-graph backups live alongside the (possibly overridden)
    // knowledge DB so an isolated `knowledge.db_path` keeps backups inside
    // the same temp/alternate directory instead of escaping to the shared
    // data dir (issue #233).
    let backup_dir = kg_db_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join("backups");
    let mut knowledge_graph = mimir_knowledge::KnowledgeGraph::init(&kg_db_path).await?;

    // Inject the default OSM Nominatim geocoder so the entity-locations
    // write path (Phase 3 S3 / #193) can fill the missing half of a
    // location (address -> coords or coords -> address). The backend does
    // no network work until a location fact is actually processed, so this
    // is cheap at startup. A config toggle / self-hosted endpoint can
    // follow; for now the policy-compliant public-instance defaults apply.
    // Construction only fails if the HTTP client or rate limiter cannot be
    // built, in which case geocoding is disabled (locations still persist
    // with whatever data the producer supplied) rather than aborting start.
    // The geocoder is shared between the knowledge graph (entity-locations
    // write path, S3 / #193) and the connector supervisor (Photos place
    // extraction, C2 / #196), so build the Arc once and hand the same
    // instance to both.
    let mut shared_geocoder: Option<Arc<dyn Geocoder>> = None;
    match mimir_connectors::NominatimGeocoder::with_defaults() {
        Ok(geocoder) => {
            let geocoder: Arc<dyn Geocoder> = Arc::new(geocoder);
            knowledge_graph.set_geocoder(Arc::clone(&geocoder));
            shared_geocoder = Some(geocoder);
            tracing::info!("Nominatim geocoder enabled for entity-locations write path");
        }
        Err(error) => tracing::warn!(
            "failed to initialise Nominatim geocoder; location geocoding disabled: {error}"
        ),
    }
    let knowledge_graph = Arc::new(knowledge_graph);

    // Resolve user entity from config identity.
    let user_entity_id = if cfg.identity.name.trim().is_empty() {
        tracing::warn!(
            "identity.name is empty; skipping user entity resolution. memory condensation will not run."
        );
        None
    } else {
        match knowledge_graph.search_entities(&cfg.identity.name, 1).await {
            Ok(mut results) if !results.is_empty() => {
                tracing::info!(
                    "Resolved user entity: {} (id={})",
                    results[0].entity.name,
                    results[0].entity.id
                );
                Some(results.remove(0).entity.id)
            }
            Ok(_) => {
                tracing::info!(
                    "User entity '{}' not found; creating as User type",
                    cfg.identity.name
                );
                match knowledge_graph
                    .create_entity(
                        &cfg.identity.name,
                        mimir_knowledge::models::entity::EntityType::Person,
                        &[],
                    )
                    .await
                {
                    Ok(entity) => Some(entity.id),
                    Err(e) => {
                        tracing::warn!(
                            "Failed to create user entity: {}; condensation disabled",
                            e
                        );
                        None
                    }
                }
            }
            Err(e) => {
                tracing::warn!(
                    "Failed to resolve user entity '{}': {}; condensation disabled",
                    cfg.identity.name,
                    e
                );
                None
            }
        }
    };

    // Seed identity facts for the user entity so Mimir knows the user's name.
    if let Some(uid) = user_entity_id {
        let name = cfg.identity.name.trim();
        let preferred = cfg.identity.preferred_name.trim();
        if let Err(e) = seed_identity_facts(&knowledge_graph, uid, name, preferred).await {
            tracing::warn!("Failed to seed identity facts: {}", e);
        }
    }

    // Register knowledge graph tools.
    register_tool(
        tool_registry,
        Arc::new(mimir_knowledge::KgQueryTool::new(Arc::clone(
            &knowledge_graph,
        ))),
    );
    register_tool(
        tool_registry,
        Arc::new(mimir_knowledge::KgRelatedTool::new(Arc::clone(
            &knowledge_graph,
        ))),
    );
    register_tool(
        tool_registry,
        Arc::new(mimir_knowledge::KgSearchTool::new(Arc::clone(
            &knowledge_graph,
        ))),
    );
    register_tool(
        tool_registry,
        Arc::new(mimir_knowledge::KgExpandCatalogueTool::new(Arc::clone(
            &knowledge_graph,
        ))),
    );
    register_tool(
        tool_registry,
        Arc::new(mimir_knowledge::KgFactsInCatalogueTool::new(Arc::clone(
            &knowledge_graph,
        ))),
    );
    register_tool(
        tool_registry,
        Arc::new(mimir_knowledge::RememberTool::new(Arc::clone(
            &knowledge_graph,
        ))),
    );
    register_tool(
        tool_registry,
        Arc::new(mimir_knowledge::RetrieveContextTool::new(
            Arc::clone(&knowledge_graph),
            Arc::clone(context_manager),
            Arc::clone(llm),
        )),
    );

    Ok(KnowledgeGraphInit {
        knowledge_graph,
        geocoder: shared_geocoder,
        user_entity_id,
        backup_dir,
    })
}

/// Initialise the durable job queue from the configured scheduler DB path.
pub(super) async fn init_job_queue(cfg: &Config) -> anyhow::Result<Arc<JobQueue>> {
    let jobs_db_path = mimir_core::paths::resolve_db_path(
        cfg.scheduler.db_path.clone(),
        mimir_core::paths::jobs_db_path,
    )?;
    Ok(Arc::new(JobQueue::init(&jobs_db_path).await?))
}

/// Initialise the in-memory agent runtime with the built-in agents.
pub(super) async fn init_agent_runtime() -> Arc<AgentRuntime> {
    // The LibrarianAgent is registered so it remains available for future
    // on-demand/bulk extraction, but it is no longer auto-invoked from the
    // chat route (issue #137; learning is now LLM-orchestrated via the
    // `remember` tool).
    let agent_runtime = Arc::new(AgentRuntime::new());
    agent_runtime
        .register::<mimir_knowledge::librarian::LibrarianAgent>(
            mimir_knowledge::librarian::LibrarianAgent::new(),
        )
        .await;
    agent_runtime
}

/// Initialise the background scheduler and register the daemon's system jobs:
/// knowledge optimization, memory condensation, pending-fact cleanup, and the
/// events upcoming-scan (one job per configured run time).
pub(super) async fn init_scheduler(
    cfg: &Config,
    job_queue: &Arc<JobQueue>,
    llm: &Arc<dyn LlmBackend>,
    kg: &Arc<mimir_knowledge::KnowledgeGraph>,
    activity: &Arc<AtomicU64>,
    user_entity_id: Option<i32>,
    backup_dir: std::path::PathBuf,
) -> anyhow::Result<(Arc<BackgroundScheduler>, tokio::sync::watch::Receiver<bool>)> {
    let scheduler_cfg = cfg.scheduler.clone();
    let (scheduler, scheduler_shutdown_rx) = BackgroundScheduler::new(
        Arc::clone(job_queue),
        Arc::clone(llm),
        std::time::Duration::from_secs(scheduler_cfg.debounce_seconds as u64),
        std::time::Duration::from_secs(scheduler_cfg.cooldown_seconds as u64),
    );

    // Register knowledge graph optimization job.
    let kg_for_job = Arc::clone(kg);
    let llm_for_job = Arc::clone(llm);
    let activity_for_job = Arc::clone(activity);
    let timeout_minutes = cfg.knowledge.optimization.timeout_minutes;
    let schedule_time = cfg.knowledge.optimization.schedule_time.clone();
    let pending_cleanup_retention_days = cfg.knowledge.pending_cleanup.retention_days;
    let resource_limits = optimization_resource_limits(cfg);
    let schedule =
        mimir_core::job_queue::DailySchedule::parse(&cfg.knowledge.optimization.schedule_time)?;

    let scheduler_for_opt = Arc::clone(&scheduler);
    let opt_job = Job::new(
        "knowledge.optimization",
        JobPriority::System,
        Some(schedule),
        true,
        move |_ctx: JobContext| {
            let kg = Arc::clone(&kg_for_job);
            let llm = Arc::clone(&llm_for_job);
            let activity = Arc::clone(&activity_for_job);
            let scheduler = Arc::clone(&scheduler_for_opt);
            let backup_dir = backup_dir.clone();
            let timeout = timeout_minutes;
            let schedule_time = schedule_time.clone();
            let pending_cleanup_retention = pending_cleanup_retention_days;
            Box::pin(async move {
                let opt_config = mimir_knowledge::optimization::OptimizationConfig {
                    backup_dir,
                    timeout_minutes: timeout,
                    schedule_time,
                    pending_cleanup_retention_days: pending_cleanup_retention,
                };
                let runner = mimir_knowledge::optimization::OptimizationRunner::new(
                    &kg,
                    opt_config,
                    Some(llm),
                );
                let five_minutes = chrono::Duration::minutes(5);
                runner
                    .run_all_with_callback(
                        || {
                            let last = chrono::DateTime::from_timestamp(
                                activity.load(Ordering::Relaxed) as i64,
                                0,
                            )
                            .unwrap_or_else(|| Utc::now() - chrono::Duration::days(1));
                            Utc::now() - last < five_minutes
                        },
                        || async move {
                            scheduler.submit(DaemonJob::MemoryCondensation).await;
                        },
                    )
                    .await
                    .map_err(|e| mimir_core::job_queue::JobError::Handler(e.to_string()))?;
                Ok(())
            })
        },
    )
    .with_resource_limits(resource_limits);
    job_queue.register(opt_job).await?;

    // Register memory condensation job.
    let kg_for_cond = Arc::clone(kg);
    let llm_for_cond = Arc::clone(llm);
    let char_limit = cfg.memory.char_limit as usize;
    let top_n = cfg.memory.condensation_top_n as usize;

    let cond_job = Job::new(
        "memory.condensation",
        JobPriority::System,
        None,
        true,
        move |_ctx: JobContext| {
            let kg = Arc::clone(&kg_for_cond);
            let llm = Arc::clone(&llm_for_cond);
            let uid = user_entity_id;
            let limit = char_limit;
            let top_n = top_n;
            Box::pin(async move {
                if let Some(subject_id) = uid {
                    let condenser = mimir_knowledge::condensation::MemoryCondenser::new(
                        kg, llm, subject_id, limit, top_n,
                    );
                    condenser
                        .run()
                        .await
                        .map_err(|e| mimir_core::job_queue::JobError::Handler(e.to_string()))?;
                } else {
                    tracing::debug!("memory.condensation: no user entity configured; skipping");
                }
                Ok(())
            })
        },
    );
    job_queue.register(cond_job).await?;

    // Register the pending sensitive-fact auto-cleanup job (issue #141).
    // Hard-deletes facts still awaiting confirmation past the configured
    // retention window, writing a `Rejected` audit entry per fact. Daily,
    // idle-gated; shares its deletion logic with the optimization runner's
    // `pending_confirmation_cleanup` pass via `delete_stale_pending`.
    let kg_for_cleanup = Arc::clone(kg);
    let cleanup_retention_days = cfg.knowledge.pending_cleanup.retention_days;
    let cleanup_schedule =
        mimir_core::job_queue::DailySchedule::parse(&cfg.knowledge.pending_cleanup.schedule_time)?;
    let cleanup_job = Job::new(
        "knowledge.pending_cleanup",
        JobPriority::System,
        Some(cleanup_schedule),
        true,
        move |_ctx: JobContext| {
            let kg = Arc::clone(&kg_for_cleanup);
            let retention = cleanup_retention_days;
            Box::pin(async move {
                let deleted = kg
                    .delete_stale_pending(retention)
                    .await
                    .map_err(|e| mimir_core::job_queue::JobError::Handler(e.to_string()))?;
                if deleted > 0 {
                    tracing::info!(
                        "knowledge.pending_cleanup: deleted {deleted} stale pending fact(s)"
                    );
                }
                Ok(())
            })
        },
    );
    job_queue.register(cleanup_job).await?;

    // Register the events & reminders upcoming-scan job (issue #74).
    // One scheduled job per configured run time; each shares the same
    // deterministic scan handler (derive overlays + auto-complete +
    // recurring advancement).
    let kg_for_events = Arc::clone(kg);
    let events_horizon = cfg.knowledge.events.horizon_days as i64;
    for (idx, time_str) in cfg.knowledge.events.schedule_times.iter().enumerate() {
        let events_schedule = mimir_core::job_queue::DailySchedule::parse(time_str)?;
        let job_id = format!("events.upcoming_scan_{idx}");
        let kg_clone = Arc::clone(&kg_for_events);
        let horizon = events_horizon;
        let events_job = Job::new(
            job_id,
            JobPriority::System,
            Some(events_schedule),
            true,
            move |_ctx: JobContext| {
                let kg = Arc::clone(&kg_clone);
                let horizon = horizon;
                Box::pin(async move {
                    let summary = kg
                        .run_events_scan(horizon)
                        .await
                        .map_err(|e| mimir_core::job_queue::JobError::Handler(e.to_string()))?;
                    if summary.derived + summary.completed + summary.advanced > 0 {
                        tracing::info!(
                            "events.upcoming_scan: derived {} completed {} advanced {}",
                            summary.derived,
                            summary.completed,
                            summary.advanced
                        );
                    }
                    Ok(())
                })
            },
        );
        job_queue.register(events_job).await?;
    }

    Ok((scheduler, scheduler_shutdown_rx))
}

/// Best-effort resource limits for the knowledge-optimization job, derived
/// from `[knowledge.optimization]` (issue #91). A `cpu_cores` of 0 means "no
/// affinity limit" and is mapped to `None`.
pub(super) fn optimization_resource_limits(
    cfg: &Config,
) -> mimir_core::job_queue::JobResourceLimits {
    mimir_core::job_queue::JobResourceLimits {
        cpu_cores: (cfg.knowledge.optimization.cpu_cores > 0)
            .then_some(cfg.knowledge.optimization.cpu_cores),
        nice_level: Some(cfg.knowledge.optimization.nice_level),
        memory_limit_bytes: cfg
            .knowledge
            .optimization
            .memory_limit_mb
            .map(|mb| u64::from(mb) * 1024 * 1024),
    }
}

/// Initialise the connector framework: register the built-in backend factories
/// (feature-gated), wire the supervisor with the shared services, and restore
/// `Active` runners from the connectors table.
pub(super) async fn init_connector_framework(
    cfg: &Config,
    kg: &Arc<mimir_knowledge::KnowledgeGraph>,
    llm: &Arc<dyn LlmBackend>,
    geocoder: Option<Arc<dyn Geocoder>>,
    shutdown_tx: &tokio::sync::watch::Sender<bool>,
) -> anyhow::Result<(Arc<ConnectorRegistry>, Arc<ConnectorSupervisor>)> {
    // Build the registry of built-in connector backends, gated by the
    // mimir-connectors cargo features. The mock factory is registered
    // under `cfg(test)` and the `mock-connector` feature so a release
    // daemon never advertises a test connector unless explicitly built
    // with the feature (the CLI e2e suite enables it). Each backend
    // string matches what connectors persist on their `connectors.backend`
    // row (e.g. "local", "caldav", "imap").
    let connector_registry = Arc::new(ConnectorRegistry::new());
    #[cfg(feature = "photos")]
    {
        use mimir_connectors::PhotosConnectorFactory;
        if let Err(e) = connector_registry.register(
            mimir_knowledge::models::enums::ConnectorType::Photos,
            "local".to_string(),
            PhotosConnectorFactory,
        ) {
            tracing::warn!("Failed to register Photos connector factory: {e}");
        }
    }
    #[cfg(feature = "calendar")]
    {
        use mimir_connectors::CalendarConnectorFactory;
        if let Err(e) = connector_registry.register(
            mimir_knowledge::models::enums::ConnectorType::Calendar,
            "caldav".to_string(),
            CalendarConnectorFactory,
        ) {
            tracing::warn!("Failed to register Calendar connector factory: {e}");
        }
    }
    #[cfg(feature = "gmail")]
    {
        use mimir_connectors::EmailConnectorFactory;
        if let Err(e) = connector_registry.register(
            mimir_knowledge::models::enums::ConnectorType::Gmail,
            "imap".to_string(),
            EmailConnectorFactory,
        ) {
            tracing::warn!("Failed to register Email connector factory: {e}");
        }
    }
    #[cfg(any(test, feature = "mock-connector"))]
    {
        use mimir_connectors::MockConnectorFactory;
        if let Err(e) = connector_registry.register(
            mimir_knowledge::models::enums::ConnectorType::Gmail,
            "test".to_string(),
            MockConnectorFactory,
        ) {
            tracing::warn!("Failed to register mock connector factory: {e}");
        }
    }

    // Wire the supervisor with the shared services the connector backends
    // need at construction: the secret store (F10 / #187, so Email/Calendar
    // can read credentials immediately), the geocoder (C2 / #196), the user
    // identity (C4 / #198, so Calendar authors `user has_event`), and the
    // shared LLM backend (C7 / #201, so the Email prose-extraction layer
    // routes through the system queue). The shutdown watch is the
    // daemon-wide signal so `mimir stop` drains the runners too. Builders
    // consume `self`, so the chain is assembled on the owned value before
    // it is shared behind `Arc`.
    //
    // The secret store is best-effort: `FileSecretStore::new()` resolves
    // the secrets directory and may fail on hosts without a writable home
    // (or in sandboxed tests). A missing store does not abort startup —
    // connectors that need credentials will surface the gap at
    // authentication and the user can reconfigure. This keeps the daemon
    // start path robust and avoids writing to a real secrets directory
    // during tests that exercise the connector routes with the mock
    // connector (which needs no secrets).
    let connector_supervisor = ConnectorSupervisor::new(
        Arc::clone(&connector_registry),
        Arc::clone(kg),
        SupervisorConfig::default(),
        shutdown_tx.subscribe(),
    );
    let connector_supervisor = match mimir_connectors::FileSecretStore::new() {
        Ok(store) => connector_supervisor.with_secret_store(
            std::sync::Arc::new(store) as std::sync::Arc<dyn mimir_connectors::SecretStore>
        ),
        Err(error) => {
            tracing::warn!("FileSecretStore unavailable; connector credentials disabled: {error}");
            connector_supervisor
        }
    };
    let connector_supervisor = match geocoder {
        Some(geocoder) => connector_supervisor.with_geocoder(geocoder),
        None => connector_supervisor,
    };
    let connector_supervisor = connector_supervisor
        .with_user_identity(cfg.identity.name.clone())
        .with_llm_backend(Arc::clone(llm));
    let connector_supervisor = Arc::new(connector_supervisor);

    // Spawn a runner for every connector row already in `Active` state so
    // restarts resume syncs. `restore` is best-effort: a failure to restore
    // one connector (e.g. a missing factory for a stored backend) is
    // logged inside the supervisor path and must not abort daemon startup —
    // the user can reconfigure via the routes.
    match Arc::clone(&connector_supervisor).restore().await {
        Ok(count) => {
            if count > 0 {
                tracing::info!("Restored {count} connector runner(s) at startup");
            }
        }
        Err(error) => tracing::warn!("Connector supervisor restore failed: {error}"),
    }

    Ok((connector_registry, connector_supervisor))
}

impl AppState {
    /// Build `AppState` from the global [`ReloadableConfig`].
    pub async fn from_config(
        config: Arc<ReloadableConfig>,
    ) -> anyhow::Result<(Self, tokio::sync::watch::Receiver<bool>)> {
        let llm_client: Arc<dyn LlmBackend> =
            Arc::new(LlmClient::new(config.snapshot().await.llm.clone()).await?);
        Self::from_config_with_llm(config, llm_client).await
    }

    /// Build `AppState` from [`ReloadableConfig`] with an injected LLM backend.
    ///
    /// Primarily useful for tests that need to supply a [`MockLlmClient`](mimir_core::llm::mock::MockLlmClient)
    /// without relying on sentinel strings or config hacks.
    ///
    /// Startup order is a composition of the per-subsystem init helpers:
    /// context manager → tool registry → knowledge graph → job queue → agent
    /// runtime → background scheduler → connector framework.
    pub async fn from_config_with_llm(
        config: Arc<ReloadableConfig>,
        llm_client: Arc<dyn LlmBackend>,
    ) -> anyhow::Result<(Self, tokio::sync::watch::Receiver<bool>)> {
        let cfg = config.snapshot().await;

        let context_manager = init_context_manager(&cfg).await?;
        let (shutdown_tx, _shutdown_rx) = tokio::sync::watch::channel(false);
        let tool_registry = init_tool_registry(&context_manager);
        let kg_init =
            init_knowledge_graph(&cfg, &tool_registry, &context_manager, &llm_client).await?;
        let job_queue = init_job_queue(&cfg).await?;
        let agent_runtime = init_agent_runtime().await;
        let last_user_activity = Arc::new(AtomicU64::new(0));
        let (scheduler, scheduler_shutdown_rx) = init_scheduler(
            &cfg,
            &job_queue,
            &llm_client,
            &kg_init.knowledge_graph,
            &last_user_activity,
            kg_init.user_entity_id,
            kg_init.backup_dir,
        )
        .await?;
        let (connector_registry, connector_supervisor) = init_connector_framework(
            &cfg,
            &kg_init.knowledge_graph,
            &llm_client,
            kg_init.geocoder,
            &shutdown_tx,
        )
        .await?;

        Ok((
            Self {
                llm_client,
                context_manager,
                config,
                session_locks: Arc::new(DashMap::new()),
                start_time: Instant::now(),
                endpoint: cfg.llm.endpoint.clone(),
                model: cfg.llm.model.clone(),
                shutdown_tx,
                model_override_cache: Arc::new(DashMap::new()),
                tool_registry,
                knowledge_graph: kg_init.knowledge_graph,
                job_queue,
                agent_runtime,
                scheduler,
                last_user_activity,
                user_entity_id: kg_init.user_entity_id,
                connector_registry,
                connector_supervisor,
            },
            scheduler_shutdown_rx,
        ))
    }
}
