//! CLI handlers for the `kb` command group.
//!
//! One module per concern: [`query`] for reading/editing facts, [`profile`]
//! for entity profiles and audit trails, [`maintenance`] for forget/restore/
//! trash/optimization/category/pending flows. Shared helpers live here.

use chrono::{DateTime, Utc};
use mimir_client::MimirClient;

mod maintenance;
mod profile;
mod query;
#[cfg(test)]
mod tests;

pub use maintenance::{
    KbForgetInput, handle_kb_category, handle_kb_confirm, handle_kb_forget, handle_kb_optimization,
    handle_kb_pending, handle_kb_reject, handle_kb_restore, handle_kb_trash,
};
pub use profile::{handle_kb_audit, handle_kb_profile};
pub use query::{handle_kb_browse, handle_kb_edit, handle_kb_query, handle_kb_show};

#[allow(dead_code)]
fn parse_datetime(s: &str) -> Option<DateTime<Utc>> {
    if let Ok(dt) = s.parse::<DateTime<Utc>>() {
        return Some(dt);
    }
    if let Ok(d) = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return d
            .and_hms_opt(0, 0, 0)
            .map(mimir_core::job_queue::DailySchedule::naive_to_utc_local);
    }
    for fmt in [
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%d %H:%M:%S%.f",
    ] {
        if let Ok(ndt) = chrono::NaiveDateTime::parse_from_str(s, fmt) {
            return Some(mimir_core::job_queue::DailySchedule::naive_to_utc_local(
                ndt,
            ));
        }
    }
    None
}

// ------------------------------------------------------------------
// Shared error helper
// ------------------------------------------------------------------

fn exit_with_error(msg: impl std::fmt::Display) -> ! {
    eprintln!("Error: {}", msg);
    std::process::exit(1);
}

fn make_client(base_url: &str) -> MimirClient {
    MimirClient::new(base_url)
}

// ------------------------------------------------------------------
// Confidence color helper
// ------------------------------------------------------------------

fn confidence_color(conf: f32) -> colored::Color {
    if conf > 0.9 {
        colored::Color::Green
    } else if conf >= 0.7 {
        colored::Color::Yellow
    } else {
        colored::Color::Red
    }
}

// ------------------------------------------------------------------
// truncate helper
// ------------------------------------------------------------------

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let truncated: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{truncated}…")
}
