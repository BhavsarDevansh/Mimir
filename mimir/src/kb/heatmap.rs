//! `mimir kb heatmap` — knowledge-density visualization (issue #69).
//!
//! Fetches the daemon's heatmap aggregates (`GET /kb/heatmap`) and renders
//! them as terminal bar charts, or dumps the raw JSON with `--json`.

use mimir_api_types::{HeatmapBandRow, HeatmapCountRow, HeatmapResponse, HeatmapTemporalRow};

use super::{exit_with_error, make_client};

/// Bar-chart width (in block characters) for every heatmap section.
const BAR_WIDTH: usize = 20;

/// Render one bar line: label padded to `label_width`, a scaled block bar,
/// then the count with thousands separators.
pub(crate) fn bar_line(
    label: &str,
    count: i64,
    max: i64,
    width: usize,
    label_width: usize,
) -> String {
    let filled = if max <= 0 {
        0
    } else {
        ((count * width as i64) / max).clamp(0, width as i64) as usize
    };
    format!(
        "{:<label_width$} {} {}",
        label,
        "█".repeat(filled),
        format_count(count)
    )
}

/// Format a count with thousands separators (`1247` → `1,247`).
fn format_count(count: i64) -> String {
    let digits = count.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// Render one labelled section: heading plus one bar line per `(label, count)`.
fn render_section(heading: &str, rows: Vec<(String, i64)>) -> String {
    let mut out = format!("{heading}\n");
    if rows.is_empty() {
        out.push_str("  (none)\n");
        return out;
    }
    let max = rows.iter().map(|(_, count)| *count).max().unwrap_or(0);
    let label_width = rows
        .iter()
        .map(|(label, _)| label.chars().count())
        .max()
        .unwrap_or(1);
    for (label, count) in rows {
        out.push_str(&format!(
            "  {}\n",
            bar_line(&label, count, max, BAR_WIDTH, label_width)
        ));
    }
    out
}

/// Render a ranked (entity | predicate) section with its heading.
fn render_ranked(heading: &str, rows: &[HeatmapCountRow]) -> String {
    let rows: Vec<(String, i64)> = rows.iter().map(|r| (r.name.clone(), r.count)).collect();
    render_section(heading, rows)
}

/// Render the temporal (month bucket) section.
fn render_temporal(rows: &[HeatmapTemporalRow]) -> String {
    let rows: Vec<(String, i64)> = rows.iter().map(|r| (r.period.clone(), r.count)).collect();
    render_section("Facts per Month:", rows)
}

/// Render the confidence-band distribution section (fixed band order).
fn render_bands(rows: &[HeatmapBandRow]) -> String {
    let rows: Vec<(String, i64)> = rows.iter().map(|r| (r.label.clone(), r.count)).collect();
    render_section("Confidence Distribution:", rows)
}

/// Render the full heatmap as terminal text.
pub(crate) fn render_heatmap(resp: &HeatmapResponse) -> String {
    let mut out = String::from("Knowledge Graph Heatmap\n");
    out.push_str("========================\n");
    out.push_str(&format!(
        "Facts: {}   Entities: {}   Avg Confidence: {:.2}\n\n",
        format_count(resp.facts),
        format_count(resp.entities),
        resp.avg_confidence
    ));
    out.push_str(&render_ranked("Top Entities by Facts:", &resp.top_entities));
    out.push('\n');
    out.push_str(&render_ranked("Predicates by Facts:", &resp.predicates));
    out.push('\n');
    out.push_str(&render_temporal(&resp.temporal));
    out.push('\n');
    out.push_str(&render_bands(&resp.confidence_bands));
    out
}

/// `mimir kb heatmap [--json]` — fetch and render the density snapshot.
pub async fn handle_kb_heatmap(json: bool, base_url: &str) {
    let client = make_client(base_url);
    match client.kb_heatmap().await {
        Ok(resp) => {
            if json {
                println!("{}", serde_json::to_string_pretty(&resp).unwrap());
            } else {
                print!("{}", render_heatmap(&resp));
            }
        }
        Err(e) => exit_with_error(e),
    }
}
