//! Durable Email-connector state: the terminal LLM-extraction failure
//! ledger (issues #262, #386) plus the buffered iMIP `CANCEL` tombstones
//! (issue #283).
//!
//! Since the hooks engine (issue #386), retry of a failed prose extraction
//! is owned by the hook runner: the `connector_item.remember` hook re-enqueues
//! a failed instance with time-based exponential backoff up to the
//! connector's `llm_extraction_max_attempts` budget. The ledger's role
//! shrank to the durable terminal state:
//!
//! - **Terminal failures.** When the hook's retry budget is exhausted the
//!   handler records the message here with the last error. The message is
//!   never re-processed (the extraction loop skips it), the failure
//!   surfaces via [`crate::connector::HealthStatus::Degraded`] (see
//!   `EmailConnector::health`), and the records are retained (bounded by
//!   [`MAX_TERMINAL_FAILURES`], oldest dropped first) for audit.
//! - **Durability.** The ledger is persisted by the supervisor via
//!   [`Connector::durable_state`](crate::connector::Connector::durable_state)
//!   after each successful extraction cycle and re-injected at construction
//!   as the `__durable_state` config key. Persistence is write-through: the
//!   supervisor only acknowledges the ledger as clean after the database
//!   write succeeds, so a failed write keeps the ledger dirty and the next
//!   cycle re-persists it. Because hook runs are asynchronous to the
//!   supervisor cycle, each snapshot carries a version and
//!   [`mark_persisted`](ProseRetryLedger::mark_persisted) only clears the
//!   dirty flag when no mutation happened between the snapshot and the
//!   persist — a terminal failure recorded mid-cycle is re-persisted next
//!   cycle instead of silently lost.
//! - **Legacy migration.** Pending retries persisted by the pre-hooks
//!   engine are drained at construction so their raw bytes re-stage into
//!   the buffer and are re-enqueued as hooks on the next cycle; the new
//!   code path writes the same map as a **durable queue-overflow**: when
//!   the `connector_item.remember` hook's pending queue is full (issue
//!   #442 review) `extract` records the staged email here (raw bytes
//!   base64-encoded, bounded by [`MAX_PENDING_OVERFLOW`]) instead of
//!   dropping it, so a full queue can never advance the IMAP cursor past a
//!   message whose LLM extraction was not enqueued. Every extraction cycle
//!   re-stages the recorded payloads into the buffer and re-attempts the
//!   enqueue, and a restart re-stages them at construction (the cursor has
//!   already advanced past the message, so an IMAP re-fetch would not find
//!   it again).
//! - **iMIP tombstones.** The ledger also carries the connector's buffered
//!   iMIP `CANCEL` references (issue #283): each cancelled VEVENT's
//!   namespaced reference is staged during `extract`, reported via
//!   [`Connector::extract_deletions`](crate::connector::Connector::extract_deletions),
//!   and persisted with the ledger, so a restart between `extract` and the
//!   supervisor's deletion pass re-reports the removals instead of losing
//!   them (the CANCEL email is consumed by `extract` and the IMAP cursor
//!   has already advanced past it, so the message is never re-fetched).
//!
//! The ledger is pure policy over the extraction loop: it never touches the
//! knowledge graph or the IMAP session (the crate is sqlx-free and the
//! deterministic cascade layers are unaffected).

use std::collections::BTreeMap;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::connector::HealthStatus;

/// Default maximum LLM prose-extraction attempts per message.
pub(crate) const DEFAULT_MAX_LLM_EXTRACTION_ATTEMPTS: u8 = 3;

/// Cap on retained terminal-failure records so a mailbox where every prose
/// message permanently fails cannot grow the durable ledger without bound.
/// The oldest records are dropped first.
pub(crate) const MAX_TERMINAL_FAILURES: usize = 64;

/// Cap on retained queue-overflow entries so a mailbox staged faster than
/// the hook queue drains cannot grow the durable ledger without bound
/// (the server registers `connector_item.remember` with a 1024-instance
/// pending cap, so the durable overflow mirrors the queue it backs up).
/// When the cap is exceeded the excess entries are dropped in
/// deterministic (raw-reference) order with a warning.
pub(crate) const MAX_PENDING_OVERFLOW: usize = 1024;

/// Combine a successful service probe with the retry ledger's terminal
/// backlog: terminal LLM-extraction failures make the connector
/// [`HealthStatus::Degraded`] (reachable, but repeated per-message failures)
/// so the state surfaces in connector health. Other probe outcomes pass
/// through unchanged.
pub(crate) fn health_with_terminal(probe: HealthStatus, terminal_failures: usize) -> HealthStatus {
    if probe == HealthStatus::Online && terminal_failures > 0 {
        HealthStatus::Degraded
    } else {
        probe
    }
}

/// A pending retry awaiting re-staging: either a legacy entry persisted by
/// the pre-hooks engine (issue #386, drained once at construction) or a
/// queue-overflow entry written by the hooks-engine path when the
/// `connector_item.remember` pending queue was full (issue #442 review).
/// Every extraction cycle drains these entries so their raw bytes re-stage
/// into the buffer and are re-enqueued as hooks.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct PendingProse {
    /// IMAP `UIDVALIDITY` of the mailbox the message was fetched from.
    pub uid_validity: u32,
    /// IMAP UID of the message within that `UIDVALIDITY` epoch.
    pub uid: u32,
    /// Raw RFC 822 bytes, base64-encoded so the payload survives a restart
    /// without an IMAP re-fetch (the cursor has already advanced past the
    /// message). `None` when the payload was not persisted (oversized or
    /// corrupt); such entries are dropped when re-staged.
    pub raw_b64: Option<String>,
    /// Failed attempts so far (1-based; `0` for a queue-overflow entry,
    /// which was rejected before its first attempt).
    pub attempts: u8,
    /// Last failure reason (`"queue full"` for an overflow entry).
    pub last_error: String,
}

impl PendingProse {
    /// The `UIDVALIDITY`-qualified provenance reference (`{uid_validity}:{uid}`),
    /// matching the `raw_reference` the extraction cascade emits.
    pub fn raw_ref(&self) -> String {
        format!("{}:{}", self.uid_validity, self.uid)
    }

    /// The staged IMAP item this entry carries, or `None` when the persisted
    /// payload is absent or corrupt (the item is then settled and dropped).
    /// Shared by construction re-staging and the per-cycle overflow drain so
    /// both re-stage the same way.
    pub(crate) fn into_staged_item(self) -> Option<crate::email::imap::RawEmail> {
        Some(crate::email::imap::RawEmail {
            uid: self.uid,
            uid_validity: self.uid_validity,
            internal_date: None,
            raw: self.raw()?,
        })
    }

    /// Decoded raw RFC 822 bytes, or `None` when the persisted payload is
    /// absent or corrupt (the item is then settled and dropped).
    pub fn raw(&self) -> Option<Vec<u8>> {
        self.raw_b64
            .as_deref()
            .and_then(|b64| STANDARD.decode(b64).ok())
    }
}

/// A message whose retry budget is exhausted: permanently skipped, with the
/// reason recorded for surfacing (`Degraded` health) and audit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct TerminalProseFailure {
    pub uid_validity: u32,
    pub uid: u32,
    /// Attempts consumed before the terminal failure.
    pub attempts: u8,
    /// Final failure reason.
    pub last_error: String,
}

impl TerminalProseFailure {
    pub fn raw_ref(&self) -> String {
        format!("{}:{}", self.uid_validity, self.uid)
    }
}

/// The durable state of one Email connector instance: the bounded
/// terminal-failure ledger plus the buffered iMIP `CANCEL` tombstones.
///
/// Serialised to JSON for the `connectors.durable_state` column (via
/// [`durable_json`](Self::durable_json)); `dirty` tracks changes since the
/// last successful persist and is never serialised. The supervisor calls
/// [`mark_persisted`](Self::mark_persisted) only after the database write
/// succeeds, so a failed write leaves the ledger dirty and the next cycle
/// re-persists it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub(crate) struct ProseRetryLedger {
    /// Pending retries keyed by raw reference (`{uid_validity}:{uid}`):
    /// legacy pre-hooks entries (issue #386) plus queue-overflow entries
    /// written when the hook's pending queue is full (issue #442 review).
    /// Serialised under the historical `pending` key so a pre-hooks
    /// durable state parses unchanged.
    pending: BTreeMap<String, PendingProse>,
    /// Terminally failed messages (oldest first, capped).
    terminal: Vec<TerminalProseFailure>,
    /// Buffered iMIP `CANCEL` references awaiting the supervisor's deletion
    /// pass (issue #283): `extract_invites` stages each cancelled event's
    /// namespaced reference here, `Connector::extract_deletions` reports it,
    /// and the supervisor trashes the facts this instance authored for that
    /// `raw_reference` (the #247 tombstone machinery) before acknowledging.
    /// Persisted with the ledger so a restart between `extract` and the
    /// deletion pass re-reports the removals instead of losing them.
    tombstones: Vec<String>,
    /// Whether the ledger changed since the last successful persist.
    #[serde(skip)]
    dirty: bool,
    /// Mutation counter guarding the snapshot/persist race: hook runs are
    /// asynchronous to the supervisor cycle, so a mutation between
    /// [`durable_json`](Self::durable_json) and
    /// [`mark_persisted`](Self::mark_persisted) must keep the ledger dirty.
    #[serde(skip)]
    version: u64,
}

impl ProseRetryLedger {
    /// Parse a persisted ledger, falling back to an empty one (with a warn)
    /// when the stored JSON is corrupt. A restored ledger is sanitised so a
    /// raw reference can never appear both pending and terminal (only
    /// reachable from hand-edited JSON), and the corrected ledger is marked
    /// dirty so the next cycle re-persists it.
    pub(crate) fn from_json(json: &str) -> Self {
        match serde_json::from_str::<Self>(json) {
            Ok(mut ledger) => {
                ledger.sanitize();
                ledger
            }
            Err(error) => {
                warn!(%error, "invalid persisted prose-retry ledger; starting fresh");
                Self::default()
            }
        }
    }

    /// Whether the message was terminally failed and must never be
    /// re-processed (issue #262).
    pub(crate) fn is_terminal(&self, raw_ref: &str) -> bool {
        terminal_contains(&self.terminal, raw_ref)
    }

    /// Buffer an iMIP `CANCEL` reference for the supervisor's deletion pass
    /// (issue #283). Marks the ledger dirty so the tombstone is persisted
    /// with the next durable-state write.
    pub(crate) fn push_tombstone(&mut self, reference: String) {
        self.tombstones.push(reference);
        self.touch();
    }

    /// The buffered iMIP `CANCEL` references awaiting the deletion pass.
    pub(crate) fn tombstones(&self) -> &[String] {
        &self.tombstones
    }

    /// Drop the references the supervisor trashed and acknowledged. Marks
    /// the ledger dirty when anything was removed so the reduced set is
    /// persisted.
    pub(crate) fn acknowledge_deletions(&mut self, deleted: &[String]) {
        let before = self.tombstones.len();
        self.tombstones.retain(|t| !deleted.contains(t));
        if self.tombstones.len() != before {
            self.touch();
        }
    }

    /// Record a terminal LLM-extraction failure for the message (issue
    /// #386: the hook runner owns retries; the ledger only records the
    /// durable terminal state so the message is never re-processed and the
    /// failure surfaces via `Degraded` health).
    pub(crate) fn record_terminal(
        &mut self,
        raw_ref: &str,
        uid_validity: u32,
        uid: u32,
        attempts: u8,
        error: String,
    ) {
        self.touch();
        self.terminal.retain(|t| t.raw_ref() != raw_ref);
        self.terminal.push(TerminalProseFailure {
            uid_validity,
            uid,
            attempts,
            last_error: error,
        });
        self.trim_terminal();
    }

    /// Serialise the ledger when it changed since the last successful
    /// persist, returning the snapshot's version alongside the JSON. `None`
    /// means nothing new to persist. Unlike a "take", this does **not**
    /// clear the dirty flag: the supervisor calls
    /// [`mark_persisted`](Self::mark_persisted) only after the database
    /// write succeeds, so a failed write leaves the ledger dirty and the
    /// next cycle re-persists it (a durable state that is only read once
    /// would be silently lost if the write failed).
    pub(crate) fn durable_json(&self) -> Option<(u64, String)> {
        if !self.dirty {
            return None;
        }
        match serde_json::to_string(self) {
            Ok(json) => Some((self.version, json)),
            Err(error) => {
                warn!(%error, "failed to serialize prose-retry ledger; durable state not persisted");
                None
            }
        }
    }

    /// Mark the ledger clean after the supervisor persisted the snapshot
    /// captured by [`durable_json`](Self::durable_json) successfully. The
    /// version guard keeps the ledger dirty when a hook handler mutated it
    /// between the snapshot and the persist (hook runs are asynchronous to
    /// the supervisor cycle), so the mutation is re-persisted next cycle
    /// instead of silently lost.
    pub(crate) fn mark_persisted(&mut self, version: u64) {
        if self.version == version {
            self.dirty = false;
        }
    }

    /// Drain pending retries (legacy pre-hooks entries, issue #386, and
    /// queue-overflow entries, issue #442 review) so the caller can
    /// re-stage their raw bytes into the buffer; the next extraction cycle
    /// re-enqueues them as hooks. Marks the ledger dirty so the drained
    /// state is persisted instead of re-staged on every cycle.
    pub(crate) fn drain_pending(&mut self) -> Vec<PendingProse> {
        if self.pending.is_empty() {
            return Vec::new();
        }
        let drained: Vec<PendingProse> = self.pending.values().cloned().collect();
        self.pending.clear();
        self.touch();
        drained
    }

    /// Record a queue-overflow entry: a staged message whose
    /// `connector_item.remember` enqueue was rejected because the hook's
    /// pending queue was full (issue #442 review). The raw RFC 822 bytes
    /// are persisted base64-encoded so a restart re-stages the message
    /// without an IMAP re-fetch (the cursor has already advanced past it).
    /// Marks the ledger dirty and caps the retained entries at
    /// [`MAX_PENDING_OVERFLOW`], dropping excess entries with a warning.
    pub(crate) fn record_overflow(
        &mut self,
        raw_ref: String,
        uid_validity: u32,
        uid: u32,
        raw: Vec<u8>,
    ) {
        self.touch();
        self.pending.insert(
            raw_ref,
            PendingProse {
                uid_validity,
                uid,
                raw_b64: Some(STANDARD.encode(raw)),
                attempts: 0,
                last_error: "queue full".to_string(),
            },
        );
        self.trim_pending();
    }

    /// Number of terminally failed messages (drives `Degraded` health).
    pub(crate) fn terminal_count(&self) -> usize {
        self.terminal.len()
    }

    /// Whether the ledger holds no pending retries, no terminal failures,
    /// and no buffered tombstones.
    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.pending.is_empty() && self.terminal.is_empty() && self.tombstones.is_empty()
    }

    /// Drop every record without marking the ledger dirty (used by `forget`,
    /// where the row — and its durable state — is deleted anyway).
    pub(crate) fn clear(&mut self) {
        self.pending.clear();
        self.terminal.clear();
        self.tombstones.clear();
        self.dirty = false;
    }

    /// Mark the ledger dirty and bump the mutation version.
    fn touch(&mut self) {
        self.dirty = true;
        self.version = self.version.wrapping_add(1);
    }

    fn trim_terminal(&mut self) {
        if self.terminal.len() > MAX_TERMINAL_FAILURES {
            let excess = self.terminal.len() - MAX_TERMINAL_FAILURES;
            self.terminal.drain(..excess);
        }
    }

    /// Bound the queue-overflow map at [`MAX_PENDING_OVERFLOW`], dropping
    /// the excess entries in deterministic key order (the map is keyed by
    /// raw reference) with a warning, so a mailbox staged faster than the
    /// hook queue drains cannot grow the persisted ledger without bound.
    fn trim_pending(&mut self) {
        if self.pending.len() > MAX_PENDING_OVERFLOW {
            let excess = self.pending.len() - MAX_PENDING_OVERFLOW;
            for _ in 0..excess {
                let _ = self.pending.pop_first().expect("over-cap pending map");
            }
            warn!(
                trimmed = excess,
                cap = MAX_PENDING_OVERFLOW,
                "queue-overflow ledger over capacity; dropping oldest entries"
            );
        }
    }

    /// Normalise a restored ledger: a raw reference may be legacy-pending
    /// or terminal, never both (the terminal record is the stricter state,
    /// so any shadowed legacy-pending entry is dropped), and the terminal
    /// list is re-capped. Marks the ledger dirty when something was removed
    /// so the correction is persisted.
    fn sanitize(&mut self) {
        let mut changed = false;
        for terminal in &self.terminal {
            changed |= self.pending.remove(&terminal.raw_ref()).is_some();
        }
        let terminal_len = self.terminal.len();
        self.trim_terminal();
        changed |= self.terminal.len() != terminal_len;
        if changed {
            self.touch();
        }
    }
}

/// Whether `raw_ref` (`{uid_validity}:{uid}`) matches a terminal record,
/// comparing numerically so gating a message never allocates a string per
/// terminal record (the list is capped at [`MAX_TERMINAL_FAILURES`]).
fn terminal_contains(terminal: &[TerminalProseFailure], raw_ref: &str) -> bool {
    let Some((validity, uid)) = raw_ref.split_once(':') else {
        return terminal.iter().any(|t| t.raw_ref() == raw_ref);
    };
    let (Ok(validity), Ok(uid)) = (validity.parse::<u32>(), uid.parse::<u32>()) else {
        return terminal.iter().any(|t| t.raw_ref() == raw_ref);
    };
    terminal
        .iter()
        .any(|t| t.uid_validity == validity && t.uid == uid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_degrades_only_when_terminal_failures_are_recorded() {
        assert_eq!(
            health_with_terminal(HealthStatus::Online, 0),
            HealthStatus::Online
        );
        assert_eq!(
            health_with_terminal(HealthStatus::Online, 1),
            HealthStatus::Degraded
        );
        assert_eq!(
            health_with_terminal(HealthStatus::Offline, 1),
            HealthStatus::Offline
        );
        assert_eq!(
            health_with_terminal(HealthStatus::AuthExpired, 2),
            HealthStatus::AuthExpired
        );
        assert_eq!(
            health_with_terminal(HealthStatus::NotConfigured, 1),
            HealthStatus::NotConfigured
        );
    }

    #[test]
    fn is_terminal_reports_only_recorded_failures() {
        let mut ledger = ProseRetryLedger::default();
        assert!(!ledger.is_terminal("17:1"));
        assert!(!ledger.dirty, "a read-only check must not dirty the ledger");
        ledger.record_terminal("17:1", 17, 1, 3, "boom".into());
        assert!(ledger.is_terminal("17:1"));
        assert!(!ledger.is_terminal("17:2"));
        assert!(
            ledger.dirty,
            "recording a terminal failure dirties the ledger"
        );
    }

    #[test]
    fn record_terminal_replaces_prior_record() {
        let mut ledger = ProseRetryLedger::default();
        ledger.record_terminal("17:1", 17, 1, 2, "e1".into());
        ledger.record_terminal("17:1", 17, 1, 3, "e2".into());
        assert_eq!(ledger.terminal.len(), 1);
        assert_eq!(ledger.terminal[0].attempts, 3);
        assert_eq!(ledger.terminal[0].last_error, "e2");
    }

    #[test]
    fn terminal_records_are_capped_oldest_first() {
        let mut ledger = ProseRetryLedger::default();
        for i in 0..(MAX_TERMINAL_FAILURES + 5) {
            ledger.record_terminal(&format!("17:{i}"), 17, i as u32, 1, format!("e{i}"));
        }
        assert_eq!(ledger.terminal.len(), MAX_TERMINAL_FAILURES);
        assert_eq!(
            ledger.terminal[0].uid, 5,
            "oldest records are dropped first"
        );
        assert_eq!(
            ledger.terminal[MAX_TERMINAL_FAILURES - 1].uid,
            (MAX_TERMINAL_FAILURES + 4) as u32,
            "newest records are retained"
        );
    }

    #[test]
    fn durable_round_trip_preserves_terminal_and_tombstones() {
        let mut ledger = ProseRetryLedger::default();
        ledger.record_terminal("99:2", 99, 2, 1, "fatal".into());
        ledger.push_tombstone("imip:dentist-1@example.com".to_string());
        let (version, json) = ledger.durable_json().expect("dirty ledger serializes");
        let mut restored = ProseRetryLedger::from_json(&json);
        assert_eq!(restored.terminal.len(), 1);
        assert_eq!(restored.terminal[0].raw_ref(), "99:2");
        assert_eq!(restored.tombstones(), &["imip:dentist-1@example.com"]);
        assert!(!restored.dirty, "restored ledger starts clean");
        assert!(restored.is_terminal("99:2"));
        restored.mark_persisted(version);
        assert_eq!(restored.durable_json(), None);
    }

    #[test]
    fn durable_json_is_dirty_tracking_until_persisted() {
        let mut ledger = ProseRetryLedger::default();
        assert_eq!(ledger.durable_json(), None, "clean ledger persists nothing");
        ledger.record_terminal("17:1", 17, 1, 1, "e".into());
        // Reading the durable state must not clear the dirty flag: the
        // supervisor acknowledges the persist only after the database write
        // succeeds, so a failed write leaves the state pending a retry.
        let (version, _json) = ledger.durable_json().expect("dirty ledger persists");
        assert!(
            ledger.durable_json().is_some(),
            "an unacknowledged read must not mark the ledger clean"
        );
        ledger.mark_persisted(version);
        assert_eq!(
            ledger.durable_json(),
            None,
            "only an acknowledged persist clears the ledger"
        );
    }

    #[test]
    fn mark_persisted_keeps_dirty_when_mutated_after_snapshot() {
        // Issue #386: hook runs are asynchronous to the supervisor cycle, so
        // a terminal failure recorded between the snapshot and the persist
        // must keep the ledger dirty and be re-persisted next cycle.
        let mut ledger = ProseRetryLedger::default();
        ledger.record_terminal("17:1", 17, 1, 1, "e1".into());
        let (version, _json) = ledger.durable_json().expect("dirty ledger persists");
        ledger.record_terminal("17:2", 17, 2, 1, "e2".into());
        ledger.mark_persisted(version);
        assert!(
            ledger.dirty,
            "a mutation after the snapshot must keep the ledger dirty"
        );
        let (new_version, json) = ledger.durable_json().expect("still dirty");
        let restored = ProseRetryLedger::from_json(&json);
        assert!(restored.is_terminal("17:2"), "late mutation is persisted");
        ledger.mark_persisted(new_version);
        assert_eq!(ledger.durable_json(), None);
    }

    #[test]
    fn tombstones_round_trip_and_track_dirty() {
        let mut ledger = ProseRetryLedger::default();
        assert_eq!(ledger.durable_json(), None, "clean ledger persists nothing");
        ledger.push_tombstone("imip:dentist-1@example.com".to_string());
        assert!(ledger.dirty, "a buffered tombstone marks the ledger dirty");
        assert_eq!(ledger.tombstones(), &["imip:dentist-1@example.com"]);
        let (version, json) = ledger.durable_json().expect("dirty ledger serializes");
        let mut restored = ProseRetryLedger::from_json(&json);
        assert_eq!(
            restored.tombstones(),
            &["imip:dentist-1@example.com"],
            "tombstones survive the durable round-trip"
        );
        restored.acknowledge_deletions(&["imip:dentist-1@example.com".to_string()]);
        assert!(
            restored.tombstones().is_empty(),
            "acknowledged tombstones are dropped"
        );
        assert!(
            restored.dirty,
            "acknowledging a removal marks the ledger dirty"
        );
        restored.mark_persisted(version);
        assert_eq!(restored.durable_json(), None);
    }

    #[test]
    fn from_json_falls_back_on_corrupt_payload() {
        let restored = ProseRetryLedger::from_json("not json");
        assert!(restored.is_empty());
    }

    #[test]
    fn from_json_sanitizes_overlapping_legacy_pending_and_terminal() {
        let mut ledger = ProseRetryLedger::default();
        ledger.record_terminal("17:1", 17, 1, 1, "boom".into());
        let (_, json) = ledger.durable_json().expect("dirty");
        let mut value: serde_json::Value = serde_json::from_str(&json).unwrap();
        // Hand-crafted overlap: the same raw reference legacy-pending and
        // terminal (only reachable from hand-edited JSON).
        value["pending"]["17:1"] = serde_json::json!({
            "uid_validity": 17,
            "uid": 1,
            "raw_b64": "cmF3",
            "attempts": 1,
            "last_error": "e",
            "skip_cycles": 1
        });
        let restored = ProseRetryLedger::from_json(&value.to_string());
        assert!(
            restored.pending.is_empty(),
            "a terminal record must shadow a legacy-pending entry for the same reference"
        );
        assert!(restored.is_terminal("17:1"));
        assert!(
            restored.dirty,
            "sanitising a restored ledger must mark it for re-persist"
        );
    }

    #[test]
    fn drain_pending_marks_dirty_and_clears() {
        let mut ledger = ProseRetryLedger::default();
        ledger.pending.insert(
            "17:1".to_string(),
            PendingProse {
                uid_validity: 17,
                uid: 1,
                raw_b64: Some(STANDARD.encode(b"raw bytes")),
                attempts: 1,
                last_error: "e".to_string(),
            },
        );
        let drained = ledger.drain_pending();
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].raw().as_deref(), Some(b"raw bytes".as_slice()));
        assert!(ledger.pending.is_empty());
        assert!(
            ledger.dirty,
            "draining pending entries must mark the ledger for re-persist"
        );
        assert!(
            ledger.drain_pending().is_empty(),
            "a second drain is a no-op"
        );
    }

    #[test]
    fn record_overflow_writes_durable_pending_entry_with_raw_bytes() {
        let mut ledger = ProseRetryLedger::default();
        ledger.record_overflow("17:42".to_string(), 17, 42, b"raw bytes".to_vec());
        assert!(
            ledger.dirty,
            "a queue-overflow record must mark the ledger for re-persist"
        );
        let (_, json) = ledger.durable_json().expect("dirty ledger persists");
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(
            value["pending"]["17:42"]["raw_b64"].is_string(),
            "the overflow payload must be persisted base64-encoded"
        );
        let mut restored = ProseRetryLedger::from_json(&json);
        let drained = restored.drain_pending();
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].raw().as_deref(), Some(b"raw bytes".as_slice()));
        assert_eq!(
            drained[0].attempts, 0,
            "an overflow entry was never attempted"
        );
    }

    #[test]
    fn record_overflow_replaces_an_existing_entry_for_the_same_reference() {
        let mut ledger = ProseRetryLedger::default();
        ledger.record_overflow("17:42".to_string(), 17, 42, b"first".to_vec());
        let (version, _) = ledger.durable_json().expect("dirty");
        ledger.record_overflow("17:42".to_string(), 17, 42, b"second".to_vec());
        let (_, json) = ledger.durable_json().expect("dirty after replacement");
        let mut restored = ProseRetryLedger::from_json(&json);
        let drained = restored.drain_pending();
        assert_eq!(drained.len(), 1, "a replacement keeps a single entry");
        assert_eq!(drained[0].raw().as_deref(), Some(b"second".as_slice()));
        ledger.mark_persisted(version);
        assert!(
            ledger.dirty,
            "the replacement bumped the version, so the snapshot is stale"
        );
    }

    #[test]
    fn overflow_records_are_capped() {
        let mut ledger = ProseRetryLedger::default();
        for uid in 0..(MAX_PENDING_OVERFLOW + 5) {
            ledger.record_overflow(format!("17:{uid}"), 17, uid as u32, b"x".to_vec());
        }
        let drained = ledger.drain_pending();
        assert_eq!(drained.len(), MAX_PENDING_OVERFLOW);
        assert!(
            drained
                .iter()
                .any(|p| p.uid == (MAX_PENDING_OVERFLOW + 4) as u32),
            "the newest overflow entries must survive the cap"
        );
    }

    #[test]
    fn clear_resets_everything_without_dirtying() {
        let mut ledger = ProseRetryLedger::default();
        ledger.record_terminal("17:1", 17, 1, 1, "e".into());
        ledger.clear();
        assert!(ledger.is_empty());
        assert!(!ledger.dirty);
        assert_eq!(ledger.durable_json(), None);
    }
}
