//! Unit tests for shared KB CLI helpers.

use super::*;
use chrono::{Local, Offset, TimeZone};

#[test]
fn parse_datetime_rfc3339() {
    let dt = parse_datetime("2020-06-15T10:30:00Z").unwrap();
    // Explicit offsets are preserved as UTC.
    assert_eq!(dt.format("%Y-%m-%d %H:%M").to_string(), "2020-06-15 10:30");
    assert_eq!(dt.offset().fix().local_minus_utc(), 0);
}

#[test]
fn parse_datetime_date_only_is_midnight() {
    let dt = parse_datetime("2020-06-15").unwrap();
    // Date-only is interpreted as local midnight, so the local wall clock
    // of the result must be 00:00:00 regardless of the host timezone.
    let local = Local.from_utc_datetime(&dt.naive_utc());
    assert_eq!(
        local.format("%Y-%m-%d %H:%M:%S").to_string(),
        "2020-06-15 00:00:00"
    );
}

#[test]
fn parse_datetime_iso_without_zone() {
    let dt = parse_datetime("2020-06-15T10:30:00").unwrap();
    // Offsetless inputs are interpreted in the local timezone, so the
    // local wall clock of the result must match the input (issue #168).
    let local = Local.from_utc_datetime(&dt.naive_utc());
    assert_eq!(
        local.format("%Y-%m-%d %H:%M").to_string(),
        "2020-06-15 10:30"
    );
}

#[test]
fn parse_datetime_explicit_offset_preserved() {
    // An explicit non-Z offset is preserved and converted to UTC.
    let dt = parse_datetime("2020-06-15T12:30:00+02:00").unwrap();
    assert_eq!(dt.format("%Y-%m-%d %H:%M").to_string(), "2020-06-15 10:30");
    assert_eq!(dt.offset().fix().local_minus_utc(), 0);
}

#[test]
fn parse_datetime_space_separator() {
    let dt = parse_datetime("2020-06-15 10:30:00").unwrap();
    let local = Local.from_utc_datetime(&dt.naive_utc());
    assert_eq!(
        local.format("%Y-%m-%d %H:%M").to_string(),
        "2020-06-15 10:30"
    );
}

#[test]
fn parse_datetime_with_fractional_seconds() {
    assert!(parse_datetime("2020-06-15T10:30:00.5").is_some());
    assert!(parse_datetime("2020-06-15 10:30:00.5").is_some());
}

#[test]
fn parse_datetime_invalid_returns_none() {
    assert!(parse_datetime("not a date").is_none());
    assert!(parse_datetime("").is_none());
    assert!(parse_datetime("2020/06/15").is_none());
}

#[test]
fn confidence_color_green_above_0_9() {
    assert_eq!(confidence_color(0.95), colored::Color::Green);
    assert_eq!(confidence_color(1.0), colored::Color::Green);
}

#[test]
fn confidence_color_yellow_at_boundary_0_7_to_0_9() {
    // 0.9 is NOT > 0.9, so it falls into the >= 0.7 branch (Yellow).
    assert_eq!(confidence_color(0.9), colored::Color::Yellow);
    assert_eq!(confidence_color(0.7), colored::Color::Yellow);
    assert_eq!(confidence_color(0.85), colored::Color::Yellow);
}

#[test]
fn confidence_color_red_below_0_7() {
    assert_eq!(confidence_color(0.69), colored::Color::Red);
    assert_eq!(confidence_color(0.0), colored::Color::Red);
    assert_eq!(confidence_color(-0.1), colored::Color::Red);
}

#[test]
fn truncate_short_input_unchanged() {
    assert_eq!(truncate("hi", 10), "hi");
    assert_eq!(truncate("", 5), "");
}

#[test]
fn truncate_exact_length_unchanged() {
    assert_eq!(truncate("abc", 3), "abc");
}

#[test]
fn truncate_long_input_gets_ellipsis() {
    let out = truncate("abcdef", 4);
    assert_eq!(out.chars().count(), 4);
    assert!(out.ends_with('…'));
    assert_eq!(out, "abc…");
}

#[test]
fn truncate_multibyte_safe() {
    let out = truncate("🎉🎉🎉🎉", 3);
    assert_eq!(out.chars().count(), 3);
    assert!(out.ends_with('…'));
}

#[test]
fn truncate_zero_max_yields_just_ellipsis_or_empty() {
    // max=0: take(0.saturating_sub(1) = 0) chars + ellipsis = just ellipsis.
    let out = truncate("abc", 0);
    assert_eq!(out, "…");
}

// ------------------------------------------------------------------
// kb heatmap rendering (issue #69)
// ------------------------------------------------------------------

use mimir_api_types::{HeatmapBandRow, HeatmapCountRow, HeatmapResponse, HeatmapTemporalRow};

fn heatmap_fixture() -> HeatmapResponse {
    HeatmapResponse {
        facts: 12_304,
        entities: 847,
        avg_confidence: 0.82,
        top_entities: vec![
            HeatmapCountRow {
                name: "devansh".to_string(),
                count: 1_247,
            },
            HeatmapCountRow {
                name: "Alice".to_string(),
                count: 623,
            },
        ],
        predicates: vec![HeatmapCountRow {
            name: "lives_in".to_string(),
            count: 431,
        }],
        temporal: vec![HeatmapTemporalRow {
            period: "2026-01".to_string(),
            count: 88,
        }],
        confidence_bands: vec![
            HeatmapBandRow {
                label: "explicit (1.0)".to_string(),
                count: 4_201,
            },
            HeatmapBandRow {
                label: "connector (0.7-1.0)".to_string(),
                count: 3_892,
            },
            HeatmapBandRow {
                label: "inference (0.4-0.7)".to_string(),
                count: 2_104,
            },
            HeatmapBandRow {
                label: "casual (<0.4)".to_string(),
                count: 1_107,
            },
        ],
    }
}

#[test]
fn render_heatmap_includes_totals_and_all_sections() {
    let out = render_heatmap(&heatmap_fixture());
    assert!(out.contains("Knowledge Graph Heatmap"));
    assert!(out.contains("Facts: 12,304"));
    assert!(out.contains("Entities: 847"));
    assert!(out.contains("Avg Confidence: 0.82"));
    assert!(out.contains("Top Entities by Facts:"));
    assert!(out.contains("Predicates by Facts:"));
    assert!(out.contains("Facts per Month:"));
    assert!(out.contains("Confidence Distribution:"));
    assert!(out.contains("devansh"));
    assert!(out.contains("1,247"));
    assert!(out.contains("explicit (1.0)"));
    assert!(out.contains("2026-01"));
}

#[test]
fn bar_line_scales_to_width_and_appends_count() {
    assert_eq!(bar_line("Alice", 100, 200, 10, 7), "Alice   █████ 100");
    assert_eq!(bar_line("Bob", 200, 200, 10, 7), "Bob     ██████████ 200");
    assert_eq!(bar_line("empty", 0, 200, 10, 7), "empty    0");
    assert_eq!(
        bar_line("thousands", 1_247, 2_000, 10, 12),
        "thousands    ██████ 1,247"
    );
}

// ------------------------------------------------------------------
// kb reset (issue #69)
// ------------------------------------------------------------------

use super::heatmap::{bar_line, render_heatmap};
use super::maintenance::{ResetFlowDeps, run_kb_reset};
use crate::connector::wizard::PromptDriver;
use mimir_api_types::ForgetResponse;
use mimir_client::MimirClient;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_partial_json, method, path},
};

/// Scripted [`PromptDriver`] for reset-flow tests: every `input` prompt
/// consumes the next canned answer.
struct ScriptedResetPrompt {
    answers: std::cell::RefCell<Vec<String>>,
}

impl ScriptedResetPrompt {
    fn new(answers: Vec<&str>) -> Self {
        Self {
            answers: std::cell::RefCell::new(answers.into_iter().map(String::from).collect()),
        }
    }
}

impl PromptDriver for ScriptedResetPrompt {
    fn select(&self, _message: &str, _options: &[String]) -> Result<usize, String> {
        panic!("reset flow never uses select")
    }

    fn input(&self, _message: &str, _default: Option<&str>) -> Result<String, String> {
        self.answers
            .borrow_mut()
            .drain(..1)
            .next()
            .ok_or_else(|| "scripted prompt ran out of answers".to_string())
    }

    fn password(&self, _message: &str) -> Result<String, String> {
        panic!("reset flow never uses password")
    }
}

async fn mount_heatmap(server: &MockServer, resp: &HeatmapResponse) {
    Mock::given(method("GET"))
        .and(path("/kb/heatmap"))
        .respond_with(ResponseTemplate::new(200).set_body_json(resp))
        .mount(server)
        .await;
}

async fn mount_forget(server: &MockServer, resp: ForgetResponse) {
    Mock::given(method("POST"))
        .and(path("/kb/facts/forget"))
        .and(body_partial_json(serde_json::json!({
            "all": true,
            "archive": false,
            "confirmation_phrase": "DELETE EVERYTHING"
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(resp))
        .mount(server)
        .await;
}

#[tokio::test]
async fn reset_wrong_phrase_aborts_without_wiping() {
    let server = MockServer::start().await;
    mount_heatmap(&server, &heatmap_fixture()).await;

    let client = MimirClient::new(server.uri());
    let deps = ResetFlowDeps {
        prompt: &ScriptedResetPrompt::new(vec!["nope"]),
        countdown_seconds: 0,
    };

    let outcome = run_kb_reset(&client, deps).await.unwrap();
    assert!(outcome.is_none());
    let hits = server.received_requests().await.unwrap();
    assert!(hits.iter().all(|r| r.url.path() == "/kb/heatmap"));
}

#[tokio::test]
async fn reset_confirmed_phrase_wipes_and_reports_backup() {
    let server = MockServer::start().await;
    mount_heatmap(&server, &heatmap_fixture()).await;
    mount_forget(
        &server,
        ForgetResponse {
            forgotten_count: 48_291,
            backup_path: Some("/tmp/backups/knowledge.db.bak-test".to_string()),
        },
    )
    .await;

    let client = MimirClient::new(server.uri());
    let deps = ResetFlowDeps {
        prompt: &ScriptedResetPrompt::new(vec!["DELETE EVERYTHING"]),
        countdown_seconds: 0,
    };

    let outcome = run_kb_reset(&client, deps).await.unwrap().unwrap();
    assert_eq!(outcome.facts_deleted, 48_291);
    assert_eq!(
        outcome.backup_path.as_deref(),
        Some("/tmp/backups/knowledge.db.bak-test")
    );

    let hits = server.received_requests().await.unwrap();
    assert_eq!(hits.len(), 2);
    assert_eq!(hits[1].url.path(), "/kb/facts/forget");
}

// ------------------------------------------------------------------
// kb Obsidian export/import (issue #62)
// ------------------------------------------------------------------

use super::obsidian::{run_kb_export, run_kb_import};
use mimir_api_types::{ExportFile, ExportResponse, ImportResponse};

fn export_fixture() -> ExportResponse {
    ExportResponse {
        files: vec![ExportFile {
            relative_path: "Devansh.md".to_string(),
            content: "# Devansh\n\n## Facts\n- allergic_to → peanuts (confidence: 1.00)\n"
                .to_string(),
        }],
        entity_count: 1,
        fact_count: 1,
        preference_count: 0,
        event_count: 0,
    }
}

async fn mount_export(server: &MockServer, resp: &ExportResponse) {
    Mock::given(method("GET"))
        .and(path("/kb/export"))
        .respond_with(ResponseTemplate::new(200).set_body_json(resp))
        .mount(server)
        .await;
}

async fn mount_import(server: &MockServer, resp: ImportResponse) {
    Mock::given(method("POST"))
        .and(path("/kb/import"))
        .respond_with(ResponseTemplate::new(200).set_body_json(resp))
        .mount(server)
        .await;
}

#[tokio::test]
async fn export_writes_bundle_files_to_target_dir() {
    let server = MockServer::start().await;
    mount_export(&server, &export_fixture()).await;

    let client = MimirClient::new(server.uri());
    let target = tempfile::tempdir().unwrap();
    run_kb_export(&client, Some(target.path().to_path_buf()), false, false)
        .await
        .unwrap();

    let written = std::fs::read_to_string(target.path().join("Devansh.md")).unwrap();
    assert!(written.starts_with("# Devansh\n"));

    let hits = server.received_requests().await.unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].url.path(), "/kb/export");
}

#[tokio::test]
async fn import_dry_run_reports_without_client_errors() {
    let server = MockServer::start().await;
    mount_import(
        &server,
        ImportResponse {
            dry_run: true,
            entities_new: 2,
            entities_updated: 0,
            facts_new: 1,
            facts_existing: 0,
            preferences_new: 0,
            preferences_updated: 0,
            dates_new: 0,
            errors: Vec::new(),
        },
    )
    .await;

    let client = MimirClient::new(server.uri());
    run_kb_import(&client, std::path::PathBuf::from("/tmp/vault"), true, false)
        .await
        .unwrap();

    let hits = server.received_requests().await.unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].url.path(), "/kb/import");
}
