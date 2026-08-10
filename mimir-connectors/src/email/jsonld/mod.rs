//! schema.org JSON-LD deterministic extraction (Phase 3, #249).
//!
//! Layer 2 of the [`super::EmailConnector::extract`] cascade: reads
//! machine-readable `schema.org` JSON-LD embedded in
//! `<script type="application/ld+json">` tags within the HTML body of
//! transactional emails and turns the high-signal types (`Order`,
//! `ParcelDelivery`, `FlightReservation`, `EventReservation`,
//! `LodgingReservation`, `Ticket`, and `ReservationPackage`) into
//! [`NormalizedFact`] clusters. No LLM — pure deterministic Rust parsing per
//! the project rule "logic in Rust, not prompts".
//!
//! # Cascade position
//!
//! Layer 1 is the iMIP calendar-invite extraction ([`super::EmailConnector`]
//! `extract_invites`, #200). This module is layer 2: it runs on the *same*
//! parsed message, scanning HTML parts for JSON-LD. Layer 3 is the C7 LLM
//! extraction (#201), which handles unstructured prose confirmations that
//! carry no machine-readable JSON-LD. By absorbing the deterministic
//! flight/booking/order facts here, C7's scope shrinks to genuinely
//! unstructured content.
//!
//! # Provenance
//!
//! Every fact carries `source_type = Connector` and the email's
//! `UIDVALIDITY`-qualified IMAP UID as `raw_reference` (matching the iMIP
//! layer and the persisted cursor format). The `connector_type` and
//! `extraction_method = StructuredParse` are set on the [`Provenance`] by the
//! supervisor's `run_cycle`, so this module only fills the per-fact fields.
//!
//! # Module layout
//!
//! - [`html`] — `<script type="application/ld+json">` block extraction.
//! - [`nodes`] — JSON-LD node traversal and type dispatch.
//! - [`facts`] — commerce extractors (orders, parcels, tickets).
//! - [`reservations`] — travel reservation extractors (flights, lodging,
//!   events, packages).
//! - [`values`] — shared value-extraction helpers.
//!
//! # Unrecognised types
//!
//! Any `@type` not in the recognised set is logged at `debug` level and
//! skipped — never guessed. This matches the acceptance criterion and the
//! connector error-handling philosophy ("malformed data → log and skip").
//!
//! [`Provenance`]: mimir_knowledge::normalize::Provenance

mod facts;
mod html;
mod nodes;
mod reservations;
mod values;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

use mail_parser::MimeHeaders;
use mimir_knowledge::normalize::NormalizedFact;
use serde_json::Value;
use tracing::debug;

use crate::email::jsonld::html::extract_jsonld_blocks;
use crate::email::jsonld::nodes::{extract_node_facts, flatten_nodes};

/// Extract JSON-LD facts from an already-parsed [`mail_parser::Message`].
///
/// Separated from [`extract_facts`] so the cascade in `extract()` (which
/// already has a parsed `Message`) can reuse the same parser instance without
/// a second parse.
pub(crate) fn extract_facts_from_message(
    user_identity: Option<&str>,
    message: &mail_parser::Message<'_>,
    raw_ref: &str,
) -> Vec<NormalizedFact> {
    let mut facts = Vec::new();
    for part in &message.parts {
        if !part.is_content_type("text", "html") {
            continue;
        }
        let Some(html) = part.text_contents() else {
            debug!(
                raw_ref,
                "text/html part had no decodable text; skipping JSON-LD scan"
            );
            continue;
        };
        for block in extract_jsonld_blocks(html) {
            match serde_json::from_str::<Value>(block) {
                Ok(value) => {
                    for node in flatten_nodes(&value) {
                        facts.extend(extract_node_facts(user_identity, node, raw_ref));
                    }
                }
                Err(err) => {
                    debug!(raw_ref, error = %err, "skipping unparseable JSON-LD block");
                }
            }
        }
    }
    facts
}
