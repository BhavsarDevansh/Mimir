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
