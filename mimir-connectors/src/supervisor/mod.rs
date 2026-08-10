//! Connector supervisor (F8 / #186, Phase 3 A2 / #203).
//!
//! [`ConnectorSupervisor`] owns one supervised task per active connector
//! instance. The module is split by concern:
//!
//! - `config` — [`SupervisorConfig`] tunables (backoff, polling bounds).
//! - `error` — [`SupervisorError`] and [`ActError`].
//! - `trigger` — manual-sync trigger types (F9 / #186).
//! - `runner` — the supervisor struct, construction, and instance
//!   spawning ([`ConnectorSupervisor::restore`], `instantiate`).
//! - `control` — runtime control: triggers, start/stop/pause/resume,
//!   action dispatch.
//! - `cycle` — the per-connector runner loop and single-cycle machinery.

mod config;
mod control;
mod cycle;
mod error;
mod runner;
mod trigger;

pub use config::SupervisorConfig;
pub use error::{ActError, SupervisorError};
pub use runner::ConnectorSupervisor;
pub use trigger::{TriggerError, TriggerOutcome};
