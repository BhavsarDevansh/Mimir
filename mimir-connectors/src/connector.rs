//! Runtime `Connector` trait definition.
//!
//! This is a minimal placeholder scaffold. The full trait, its data types
//! (`ConnectorMode`, `SyncOptions`, `HealthStatus`), and all behaviour are
//! owned by Phase 3 issue **F6**. Only the trait name and its two identity
//! accessors exist here, which is enough for the registry and mock connector
//! stubs to compile against a stable interface.

/// Runtime connector trait — the interface every service ingestion worker
/// implements.
///
/// F6 will expand this into the full async trait covering authentication,
/// health checks, polling/push mode declaration, sync, extraction, and
/// lifecycle (pause/resume/forget). Until then this placeholder keeps the
/// crate compiling and establishes the object-safe name the registry and mock
/// depend on.
pub trait Connector: Send + Sync {
    /// Stable, unique, slug-style identifier for this connector instance.
    fn id(&self) -> &str;

    /// Human-readable display name.
    fn name(&self) -> &str;
}
