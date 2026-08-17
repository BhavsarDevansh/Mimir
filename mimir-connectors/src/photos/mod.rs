//! Local-filesystem Photos connector (Phase 3 C1 / #195), gated by the
//! `photos` feature.
//!
//! The module is split by concern:
//!
//! - `config` — defaults and the `PhotosConfigDto` DTO.
//! - `cursor` — the incremental [`PhotosCursor`] + file-signature helpers.
//! - `exif` — EXIF GPS / datetime parsing.
//! - `scan` — directory scanning, `RawPhoto` staging, fact conversion.
//! - `watcher` — the debounced `notify` watcher plumbing.
//! - `sync` — the sync pipeline (initial scan + event processing).
//! - `connector` — the [`PhotosConnector`] struct, construction, and the
//!   [`Connector`] trait implementation.
//! - `factory` — [`PhotosConnectorFactory`] registration.

mod config;
mod connector;
mod cursor;
mod exif;
mod factory;
mod scan;
mod sync;
mod watcher;

pub use connector::PhotosConnector;
pub use cursor::PhotosCursor;
pub use factory::PhotosConnectorFactory;
