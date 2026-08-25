//! Canonical Obsidian line + frontmatter grammar (issue #62).
//!
//! Single source of truth for the text format: both the export renderer and
//! the import parser share these helpers so the two directions cannot drift.
//! Fact lines are `- {predicate} → {object} (attrs…)`; the Relationships
//! section additionally accepts the hand-written `- [[{object}]] — {predicate}`
//! form. Attributes are comma-separated: `since {date}` / `{date} to {date}` /
//! `{date} to present` / `until {date}` / a bare `{date}` (valid_from),
//! `confidence: N.N`, `yearly|monthly|weekly|daily`, and the event-type wire
//! names (`Birthday`, `Appointment`, `Deadline`, `Task`, `Reminder`, `Custom`).

use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};

use crate::models::enums::{EventType, RecurrenceType};

pub(crate) const SECTION_DATES: &str = "Dates";
pub(crate) const SECTION_RELATIONSHIPS: &str = "Relationships";
pub(crate) const SECTION_PREFERENCES: &str = "Preferences";
pub(crate) const SECTION_FACTS: &str = "Facts";

/// YAML frontmatter of an Obsidian entity document.
///
/// `entity_type` stays a raw string so parsing is case-insensitive via
/// `EntityType::from_str`; rendering uses the canonical wire name.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Frontmatter {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity_id: Option<i32>,
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub entity_type: Option<String>,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated: Option<String>,
}

impl Frontmatter {
    /// Render the frontmatter block (without the surrounding `---` fences).
    pub(crate) fn render(&self) -> String {
        // Struct serialisation cannot fail: every field is a plain scalar or
        // a Vec<String>.
        serde_yaml::to_string(self).expect("frontmatter serialisation cannot fail")
    }

    /// Parse a raw YAML block into frontmatter.
    pub(crate) fn parse(raw: &str) -> Result<Self, String> {
        serde_yaml::from_str(raw).map_err(|e| format!("invalid YAML frontmatter: {e}"))
    }
}

/// An object reference in a fact line: another entity or a literal value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ObsidianObject {
    /// `[[Name]]` (or a bare name in the Relationships section) — a reference
    /// to another entity, resolved against the graph on import.
    Entity(String),
    /// Plain text literal stored in `object_literal`.
    Literal(String),
}

/// One parsed fact line, shared by the Dates / Relationships / Facts sections.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ParsedFactLine {
    pub predicate: String,
    pub object: ObsidianObject,
    pub valid_from: Option<DateTime<Utc>>,
    pub valid_until: Option<DateTime<Utc>>,
    pub confidence: Option<f32>,
    pub recurrence: RecurrenceType,
    pub event_type: Option<EventType>,
}

/// One parsed preference line (`- {Category}: {key} = {value}`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedPreference {
    pub category: crate::models::preference::PreferenceCategory,
    pub key: String,
    pub value: String,
}

/// Parse one fact line from a section.
pub(crate) fn parse_fact_line(section: &str, line: &str) -> Result<ParsedFactLine, String> {
    let trimmed = line.trim();
    let rest = trimmed
        .strip_prefix("- ")
        .ok_or_else(|| format!("expected a list item (`- …`), got {trimmed:?}"))?;
    let (main, attrs) = split_trailing_attrs(rest);

    let mut parsed = ParsedFactLine {
        predicate: String::new(),
        object: ObsidianObject::Literal(String::new()),
        valid_from: None,
        valid_until: None,
        confidence: None,
        recurrence: RecurrenceType::None,
        event_type: None,
    };

    if section == SECTION_RELATIONSHIPS {
        if let Some((object, predicate)) = main.split_once(" — ") {
            // Hand-written form: `[[Alice]] — married_to`.
            parsed.predicate = predicate.trim().to_string();
            let target = object.trim();
            let inner = target
                .strip_prefix("[[")
                .and_then(|t| t.strip_suffix("]]"))
                .unwrap_or(target);
            parsed.object = ObsidianObject::Entity(parse_wiki_link(inner)?);
        } else if let Some((predicate, object)) = main.split_once(" → ") {
            parsed.predicate = predicate.trim().to_string();
            parsed.object = parse_object(object, section)?;
        } else {
            return Err(format!(
                "cannot parse relationship line {trimmed:?} (expected `predicate → object` or `[[object]] — predicate`)"
            ));
        }
    } else if let Some((predicate, object)) = main.split_once(" → ") {
        parsed.predicate = predicate.trim().to_string();
        parsed.object = parse_object(object, section)?;
    } else {
        return Err(format!(
            "cannot parse fact line {trimmed:?} (expected `predicate → object`)"
        ));
    }

    if parsed.predicate.is_empty() {
        return Err(format!("empty predicate in {trimmed:?}"));
    }

    if let Some(attrs) = attrs {
        for attr in attrs.split(',') {
            apply_attr(&mut parsed, attr)?;
        }
    }
    Ok(parsed)
}

/// Parse a `- {Category}: {key} = {value}` preference line.
pub(crate) fn parse_preference_line(line: &str) -> Result<ParsedPreference, String> {
    let trimmed = line.trim();
    let rest = trimmed
        .strip_prefix("- ")
        .ok_or_else(|| format!("expected a list item, got {trimmed:?}"))?;
    let (category, kv) = rest
        .split_once(':')
        .ok_or_else(|| format!("preference line {trimmed:?} has no category prefix"))?;
    let category = category
        .trim()
        .parse::<crate::models::preference::PreferenceCategory>()
        .map_err(|_| format!("unknown preference category {:?} in {trimmed:?}", category))?;
    let (key, value) = kv
        .split_once('=')
        .ok_or_else(|| format!("preference line {trimmed:?} has no `key = value` pair"))?;
    let key = key.trim();
    let value = value.trim();
    if key.is_empty() {
        return Err(format!("empty preference key in {trimmed:?}"));
    }
    Ok(ParsedPreference {
        category,
        key: key.to_string(),
        value: value.to_string(),
    })
}

/// Render one fact line (used by every section).
#[allow(clippy::too_many_arguments)]
pub(crate) fn render_fact_line(
    predicate: &str,
    object: &ObsidianObject,
    valid_from: Option<DateTime<Utc>>,
    valid_until: Option<DateTime<Utc>>,
    confidence: f32,
    recurrence: RecurrenceType,
    event_type: Option<EventType>,
) -> String {
    let mut attrs: Vec<String> = Vec::new();
    match (valid_from, valid_until) {
        (Some(from), Some(until)) => {
            attrs.push(format!("{} to {}", render_date(from), render_date(until)));
        }
        (Some(from), None) => attrs.push(format!("since {}", render_date(from))),
        (None, Some(until)) => attrs.push(format!("until {}", render_date(until))),
        (None, None) => {}
    }
    if let Some(event_type) = event_type {
        attrs.push(event_type.as_str().to_string());
    }
    if recurrence != RecurrenceType::None {
        attrs.push(recurrence.as_str().to_string());
    }
    attrs.push(format!("confidence: {:.2}", confidence));

    format!(
        "- {} → {} ({})",
        predicate,
        render_object(object),
        attrs.join(", ")
    )
}

/// Render a preference line.
pub(crate) fn render_preference_line(
    category: crate::models::preference::PreferenceCategory,
    key: &str,
    value: &str,
) -> String {
    format!("- {}: {} = {}", category.as_str(), key, value)
}

/// Render the wiki-link / literal form of an object.
pub(crate) fn render_object(object: &ObsidianObject) -> String {
    match object {
        ObsidianObject::Entity(name) => format!("[[{}]]", name),
        ObsidianObject::Literal(value) => value.clone(),
    }
}

/// Render a timestamp as `YYYY-MM-DD` when it is midnight UTC, else RFC 3339.
pub(crate) fn render_date(dt: DateTime<Utc>) -> String {
    let naive = dt.naive_utc();
    if naive.time() == chrono::NaiveTime::from_hms_opt(0, 0, 0).expect("midnight is valid") {
        naive.date().format("%Y-%m-%d").to_string()
    } else {
        dt.to_rfc3339()
    }
}

/// Split a line into its main part and the trailing `(attr, attr…)` group.
fn split_trailing_attrs(line: &str) -> (&str, Option<&str>) {
    if let Some(idx) = line.rfind(" (") {
        if line.ends_with(')') {
            return (&line[..idx], Some(&line[idx + 2..line.len() - 1]));
        }
    }
    (line, None)
}

fn parse_object(raw: &str, section: &str) -> Result<ObsidianObject, String> {
    let raw = raw.trim();
    if raw.starts_with("[[") && raw.ends_with("]]") {
        return Ok(ObsidianObject::Entity(parse_wiki_link(
            &raw[2..raw.len() - 2],
        )?));
    }
    if section == SECTION_RELATIONSHIPS {
        // The Relationships section's objects are entity references even
        // without wiki-link syntax (the export always renders the link form).
        return Ok(ObsidianObject::Entity(raw.to_string()));
    }
    Ok(ObsidianObject::Literal(raw.to_string()))
}

/// Extract the target of a wiki link, dropping `|display` aliases and
/// `#heading` fragments.
fn parse_wiki_link(inner: &str) -> Result<String, String> {
    let target = inner.split(['|', '#']).next().unwrap_or(inner).trim();
    if target.is_empty() {
        Err("empty wiki-link target".to_string())
    } else {
        Ok(target.to_string())
    }
}

fn apply_attr(parsed: &mut ParsedFactLine, attr: &str) -> Result<(), String> {
    let attr = attr.trim();
    if attr.is_empty() {
        return Ok(());
    }
    if let Some(value) = attr.strip_prefix("confidence:") {
        let confidence: f32 = value
            .trim()
            .parse()
            .map_err(|_| format!("invalid confidence {value:?}"))?;
        if !(0.0..=1.0).contains(&confidence) {
            return Err(format!("confidence out of range: {confidence}"));
        }
        parsed.confidence = Some(confidence);
        return Ok(());
    }
    if let Some(value) = attr.strip_prefix("since ") {
        parsed.valid_from =
            Some(parse_date(value).ok_or_else(|| format!("invalid date {value:?}"))?);
        return Ok(());
    }
    if let Some(value) = attr.strip_prefix("until ") {
        parsed.valid_until =
            Some(parse_date(value).ok_or_else(|| format!("invalid date {value:?}"))?);
        return Ok(());
    }
    for separator in [" to ", " – ", " - "] {
        if let Some((from, until)) = attr.split_once(separator) {
            parsed.valid_from =
                Some(parse_date(from.trim()).ok_or_else(|| format!("invalid date {from:?}"))?);
            parsed.valid_until = if until.trim() == "present" {
                None
            } else {
                Some(parse_date(until.trim()).ok_or_else(|| format!("invalid date {until:?}"))?)
            };
            return Ok(());
        }
    }
    if attr == "present" {
        return Ok(());
    }
    if let Ok(recurrence) = attr.parse::<RecurrenceType>() {
        parsed.recurrence = recurrence;
        return Ok(());
    }
    if let Ok(event_type) = attr.parse::<EventType>() {
        parsed.event_type = Some(event_type);
        return Ok(());
    }
    if let Some(date) = parse_date(attr) {
        parsed.valid_from = Some(date);
        return Ok(());
    }
    Err(format!("unrecognised attribute {attr:?}"))
}

/// Lenient date parser: RFC 3339, `YYYY-MM-DDTHH:MM:SS`, `YYYY-MM-DD HH:MM:SS`,
/// `YYYY-MM-DD`, `YYYY-MM`, `YYYY`, and month-name years (`Sep 2023`).
pub(crate) fn parse_date(raw: &str) -> Option<DateTime<Utc>> {
    let raw = raw.trim();
    if let Ok(dt) = DateTime::parse_from_rfc3339(raw) {
        return Some(dt.with_timezone(&Utc));
    }
    for fmt in ["%Y-%m-%dT%H:%M:%S", "%Y-%m-%d %H:%M:%S"] {
        if let Ok(dt) = NaiveDateTime::parse_from_str(raw, fmt) {
            return Some(dt.and_utc());
        }
    }
    if let Ok(date) = NaiveDate::parse_from_str(raw, "%Y-%m-%d") {
        return date.and_hms_opt(0, 0, 0).map(|t| t.and_utc());
    }
    if raw.len() == 7 && raw.as_bytes()[4] == b'-' {
        if let Ok(date) = NaiveDate::parse_from_str(&format!("{raw}-01"), "%Y-%m-%d") {
            return date.and_hms_opt(0, 0, 0).map(|t| t.and_utc());
        }
    }
    if let Ok(year) = raw.parse::<i32>() {
        if (1000..=9999).contains(&year) {
            return Utc.with_ymd_and_hms(year, 1, 1, 0, 0, 0).single();
        }
    }
    if let Some((month_raw, year_raw)) = raw.split_once(' ') {
        let Ok(year) = year_raw.parse::<i32>() else {
            return None;
        };
        if let Some(month) = month_number(month_raw) {
            if let Some(date) = NaiveDate::from_ymd_opt(year, month, 1) {
                return date.and_hms_opt(0, 0, 0).map(|t| t.and_utc());
            }
        }
    }
    None
}

/// Map a (possibly abbreviated) English month name to `1..=12`.
fn month_number(raw: &str) -> Option<u32> {
    const MONTHS: [&str; 12] = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ];
    let abbr = &raw.to_ascii_lowercase()[..raw.len().min(3)];
    MONTHS
        .iter()
        .position(|month| *month == abbr)
        .map(|index| index as u32 + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_arrow_fact_line_with_all_attributes() {
        let line = parse_fact_line(
            SECTION_FACTS,
            "- visited → [[Rome]] (2025-05-03 to 2025-05-07, confidence: 0.99)",
        )
        .unwrap();
        assert_eq!(line.predicate, "visited");
        assert_eq!(line.object, ObsidianObject::Entity("Rome".to_string()));
        assert_eq!(
            line.valid_from,
            Some(Utc.with_ymd_and_hms(2025, 5, 3, 0, 0, 0).unwrap())
        );
        assert_eq!(
            line.valid_until,
            Some(Utc.with_ymd_and_hms(2025, 5, 7, 0, 0, 0).unwrap())
        );
        assert_eq!(line.confidence, Some(0.99));
        assert_eq!(line.recurrence, RecurrenceType::None);
        assert_eq!(line.event_type, None);
    }

    #[test]
    fn parses_em_dash_relationship_form() {
        let line = parse_fact_line(
            SECTION_RELATIONSHIPS,
            "- [[Alice]] — married_to (since 2022)",
        )
        .unwrap();
        assert_eq!(line.predicate, "married_to");
        assert_eq!(line.object, ObsidianObject::Entity("Alice".to_string()));
        assert_eq!(
            line.valid_from,
            Some(Utc.with_ymd_and_hms(2022, 1, 1, 0, 0, 0).unwrap())
        );
    }

    #[test]
    fn parses_date_line_with_event_and_recurrence() {
        let line = parse_fact_line(
            SECTION_DATES,
            "- birthday → 1995-08-20 (1995-08-20, Birthday, yearly)",
        )
        .unwrap();
        assert_eq!(line.event_type, Some(EventType::Birthday));
        assert_eq!(line.recurrence, RecurrenceType::Yearly);
        assert_eq!(
            line.valid_from,
            Some(Utc.with_ymd_and_hms(1995, 8, 20, 0, 0, 0).unwrap())
        );
    }

    #[test]
    fn renders_and_reparses_round_trip() {
        let object = ObsidianObject::Entity("Rome".to_string());
        let rendered = render_fact_line(
            "visited",
            &object,
            Some(Utc.with_ymd_and_hms(2025, 5, 3, 0, 0, 0).unwrap()),
            None,
            0.99,
            RecurrenceType::None,
            None,
        );
        let parsed = parse_fact_line(SECTION_FACTS, &rendered).unwrap();
        assert_eq!(parsed.predicate, "visited");
        assert_eq!(parsed.object, object);
        assert_eq!(parsed.confidence, Some(0.99));
    }

    #[test]
    fn parses_month_name_and_year_only_dates() {
        assert_eq!(
            parse_date("Sep 2023"),
            Some(Utc.with_ymd_and_hms(2023, 9, 1, 0, 0, 0).unwrap())
        );
        assert_eq!(
            parse_date("2019"),
            Some(Utc.with_ymd_and_hms(2019, 1, 1, 0, 0, 0).unwrap())
        );
        assert_eq!(
            parse_date("2022-06"),
            Some(Utc.with_ymd_and_hms(2022, 6, 1, 0, 0, 0).unwrap())
        );
    }

    #[test]
    fn parses_preference_line() {
        let pref = parse_preference_line("- FoodPreference: favourite = Italian").unwrap();
        assert_eq!(
            pref.category,
            crate::models::preference::PreferenceCategory::FoodPreference
        );
        assert_eq!(pref.key, "favourite");
        assert_eq!(pref.value, "Italian");
    }

    #[test]
    fn rejects_unknown_attributes_and_bad_confidence() {
        assert!(parse_fact_line(SECTION_FACTS, "- a → b (frobnicate)").is_err());
        assert!(parse_fact_line(SECTION_FACTS, "- a → b (confidence: 1.5)").is_err());
        assert!(parse_fact_line(SECTION_FACTS, "a → b").is_err());
    }
}
