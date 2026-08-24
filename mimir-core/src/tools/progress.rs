//! Tool-execution progress events.
//!
//! Tools that spawn long-running sub-work — most notably `retrieve_context`,
//! which runs a multi-round retrieval agent — emit [`ToolProgress`] events so
//! streaming callers can show the individual steps to the user instead of a
//! single "working" indicator that looks frozen.

/// A progress event emitted while a tool executes.
#[derive(Debug, Clone)]
pub enum ToolProgress {
    /// A tool call started.
    Started {
        /// Snake_case tool identifier.
        name: String,
        /// Human-readable display name (e.g. "Kg Query").
        display_name: String,
    },
    /// A tool call finished with a display result.
    Finished {
        /// Snake_case tool identifier.
        name: String,
        /// Human-readable display name (e.g. "Kg Query").
        display_name: String,
        /// Compact display result (single line, truncated by the caller).
        result: String,
    },
}
