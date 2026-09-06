//! Shared composition of the daemon's structured memory view.

use std::sync::Arc;

use chrono::{DateTime, Utc};

use crate::state::AppState;

/// The rendering policy for a composed memory view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetPolicy {
    /// Preserve all memory content for user-facing inspection.
    Full,
    /// Preserve the condensed core and truncate upcoming content to fit the
    /// configured prompt budget.
    Budgeted,
}

/// Explicit lifecycle state carried by the composed view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryStatusState {
    /// Lifecycle has not been evaluated.
    Unknown,
    /// The composed view has no degraded source.
    Active,
    /// One or more memory sources failed or are unavailable.
    Degraded,
}

/// Explicit confidence state carried by the composed view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryConfidenceState {
    /// Confidence has not been evaluated at view level.
    Unknown,
    /// Condensed memory is available with no degraded source.
    Stable,
    /// Condensed-memory confidence is currently degraded.
    Degraded,
}

/// Explicit provenance state carried by the composed view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryProvenanceState {
    /// Provenance is not yet attached at view level.
    Unavailable,
    /// Provenance is linked to the composed content.
    Linked,
    /// Provenance lookup failed or is incomplete.
    Degraded,
}

/// Explicit privacy state carried by the composed view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryPrivacyState {
    /// Privacy redaction has not been evaluated at view level.
    NotEvaluated,
    /// Sensitive content has been redacted.
    Redacted,
}

/// Explicit user-control state carried by the composed view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryControlState {
    /// The user has not configured view-level pinning or deprioritization.
    NotConfigured,
    /// One or more memories are pinned.
    Pinned,
    /// One or more memories are deprioritized.
    Deprioritized,
}

/// Explicit control and safety states shared by every memory consumer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryViewStates {
    /// Aggregate lifecycle state for the composed view.
    pub status: MemoryStatusState,
    /// Aggregate confidence state for the composed view.
    pub confidence: MemoryConfidenceState,
    /// Aggregate provenance state for the composed view.
    pub provenance: MemoryProvenanceState,
    /// Aggregate privacy/redaction state for the composed view.
    pub privacy: MemoryPrivacyState,
    /// Aggregate pin/deprioritization state for the composed view.
    pub control: MemoryControlState,
}

/// Character and approximate token usage for the composed memory content.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MemoryViewUsage {
    /// Number of Unicode scalar values in the core-plus-upcoming content.
    pub char_count: usize,
    /// Configured maximum Unicode scalar values for prompt memory.
    pub char_limit: usize,
    /// Percentage of the configured character limit in use.
    pub usage_percent: f64,
    /// Approximate token estimate using four characters per token.
    pub token_estimate: usize,
    /// Whether the content is at or below the configured character limit.
    pub within_budget: bool,
}

/// A structured memory view assembled by one shared builder.
#[derive(Debug, Clone, PartialEq)]
pub struct ComposedMemoryView {
    /// Cached condensed core memory, or `None` when unavailable.
    pub core: Option<String>,
    /// Freshly rendered upcoming section, omitted when empty.
    pub upcoming: Option<String>,
    /// Request-local UTC timestamp used for rendering and budgeting.
    pub now: DateTime<Utc>,
    /// Configured number of days ahead for upcoming events.
    pub temporal_horizon_days: u8,
    /// Configured maximum Unicode scalar values for prompt memory.
    pub char_limit: usize,
    /// Whether the core-memory query was enabled and completed.
    pub core_available: bool,
    /// Whether core-memory loading failed.
    pub core_degraded: bool,
    /// Whether the upcoming-memory query completed.
    pub upcoming_available: bool,
    /// Whether upcoming-memory loading is degraded or unavailable.
    pub upcoming_degraded: bool,
    /// Explicit status, confidence, provenance, privacy, and control state.
    pub states: MemoryViewStates,
    /// Character/token usage for the composed content.
    pub usage: MemoryViewUsage,
    /// Human-readable degradation and budget warnings.
    pub warnings: Vec<String>,
    /// Full-render output used for inspection endpoints.
    pub rendered: String,
}

impl ComposedMemoryView {
    /// Return the core-plus-upcoming content without the request-local anchor.
    pub fn content(&self) -> String {
        match (&self.core, &self.upcoming) {
            (Some(core), Some(upcoming)) => combine_core_and_upcoming(core, upcoming),
            (Some(core), None) => core.clone(),
            (None, Some(upcoming)) => upcoming.clone(),
            (None, None) => String::new(),
        }
    }

    /// Render the view with the requested budget policy.
    pub fn render(&self, policy: BudgetPolicy) -> String {
        match policy {
            BudgetPolicy::Full => self.rendered.clone(),
            BudgetPolicy::Budgeted => {
                let Some(core) = self.core.as_deref().filter(|text| !text.is_empty()) else {
                    return String::new();
                };
                let upcoming = self.upcoming.as_deref().unwrap_or("");
                let content = if upcoming.is_empty() {
                    core.to_string()
                } else {
                    format!("{core}\n\n{upcoming}")
                };
                truncate_to_budget(&content, self.char_limit)
            }
        }
    }
}

fn truncate_to_budget(content: &str, limit: usize) -> String {
    if limit == 0 {
        return String::new();
    }
    if content.chars().count() <= limit {
        return content.to_string();
    }

    let (core, upcoming) = match content.split_once("\n\n") {
        Some((core, upcoming)) => (core, upcoming),
        None => {
            let truncated = take_chars(content, limit);
            return truncated.to_string();
        }
    };

    if core.chars().count() >= limit {
        return core.to_string();
    }

    let remaining = limit - core.chars().count() - 2;
    format!("{core}\n\n{}", take_chars(upcoming, remaining))
}

fn take_chars(text: &str, limit: usize) -> &str {
    let end = text
        .char_indices()
        .nth(limit)
        .map_or(text.len(), |(index, _)| index);
    &text[..end]
}

fn combine_core_and_upcoming(core: &str, upcoming: &str) -> String {
    if upcoming.is_empty() {
        core.to_string()
    } else {
        format!("{core}\n\n{upcoming}")
    }
}

fn usage_for(content: &str, limit: usize) -> MemoryViewUsage {
    let char_count = content.chars().count();
    MemoryViewUsage {
        char_count,
        char_limit: limit,
        usage_percent: if limit == 0 {
            0.0
        } else {
            (char_count as f64 / limit as f64) * 100.0
        },
        token_estimate: char_count.div_ceil(4),
        within_budget: char_count <= limit,
    }
}

/// Assemble the shared structured memory view from the knowledge graph.
pub async fn compose_memory_view(state: &Arc<AppState>) -> ComposedMemoryView {
    let config = state.config.snapshot().await;
    let now = state.knowledge_graph.now();
    let char_limit = usize::from(config.memory.char_limit);
    let temporal_horizon_days = config.memory.temporal_horizon;

    let (core_result, core_available) = if config.memory.enabled {
        (state.knowledge_graph.get_condensed_memory().await, true)
    } else {
        (Ok(None), false)
    };
    let (core, core_degraded, mut warnings) = match core_result {
        Ok(Some(text)) => (Some(text), false, Vec::new()),
        Ok(None) => {
            let warning = if core_available {
                "No stable memory is available yet.".to_string()
            } else {
                "Memory is disabled.".to_string()
            };
            (None, false, vec![warning])
        }
        Err(error) => (
            None,
            true,
            vec![format!("Condensed memory unavailable: {error}")],
        ),
    };

    let (upcoming, upcoming_degraded) = match state.user_entity_id {
        Some(user_id) => {
            match state
                .knowledge_graph
                .render_upcoming_section(user_id, i64::from(temporal_horizon_days), 10)
                .await
            {
                Ok(text) => (Some(text), false),
                Err(error) => {
                    warnings.push(format!("Upcoming memory unavailable: {error}"));
                    (Some(String::new()), true)
                }
            }
        }
        None => {
            warnings.push("No user identity is configured for upcoming memory.".to_string());
            (Some(String::new()), true)
        }
    };

    let upcoming = upcoming.filter(|text| !text.is_empty());
    let core_text = core.as_deref().unwrap_or("");
    let upcoming_text = upcoming.as_deref().unwrap_or("");
    let content = combine_core_and_upcoming(core_text, upcoming_text);
    let usage = usage_for(&content, char_limit);
    if !usage.within_budget {
        warnings.push("Composed memory exceeds the configured character budget.".to_string());
    }

    let rendered = if core.is_some() || upcoming.is_some() {
        mimir_knowledge::queries::memory::refresh_now_line(&content, now)
    } else {
        mimir_knowledge::queries::memory::refresh_now_line("No stable memory yet.", now)
    };

    ComposedMemoryView {
        core,
        upcoming,
        now,
        temporal_horizon_days,
        char_limit,
        core_available,
        core_degraded,
        upcoming_available: !upcoming_degraded,
        upcoming_degraded,
        states: MemoryViewStates {
            status: if core_degraded || upcoming_degraded {
                MemoryStatusState::Degraded
            } else {
                MemoryStatusState::Active
            },
            confidence: MemoryConfidenceState::Unknown,
            provenance: MemoryProvenanceState::Unavailable,
            privacy: MemoryPrivacyState::NotEvaluated,
            control: MemoryControlState::NotConfigured,
        },
        usage,
        warnings,
        rendered,
    }
}
