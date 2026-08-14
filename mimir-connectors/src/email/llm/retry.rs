//! Durable LLM-extraction retry ledger (issue #262).
//!
//! The Email connector's prose-extraction layer (C7 / #201) retries failed
//! LLM calls by re-staging the raw email for the next extraction cycle.
//! Before this ledger, that retry was unbounded (a persistently failing
//! message was re-attempted on every cycle forever, re-paying the LLM call)
//! and in-memory only (a restart dropped the staged message because the
//! IMAP cursor had already advanced past it).
//!
//! [`ProseRetryLedger`] closes both gaps:
//!
//! - **Bounded attempts.** Each message gets a configurable retry budget
//!   ([`DEFAULT_MAX_LLM_EXTRACTION_ATTEMPTS`] by default). After the budget
//!   is exhausted the message is marked **terminally failed** with the last
//!   error recorded; it stops consuming LLM calls and is no longer staged.
//!   Terminal failures surface via [`crate::connector::HealthStatus::Degraded`]
//!   (see `EmailConnector::health`) and are retained (bounded by
//!   [`MAX_TERMINAL_FAILURES`], oldest dropped first) for audit.
//! - **Cycle backoff.** Between attempts the message waits an exponential
//!   number of extraction cycles ([`backoff_cycles`]: 1, 2, 4, … capped at
//!   8), so a stuck message does not re-pay the LLM call on every cycle.
//! - **Durability.** The ledger (pending items with their raw RFC 822 bytes
//!   base64-encoded, plus terminal records) is persisted by the supervisor
//!   via [`Connector::durable_state`](crate::connector::Connector::durable_state)
//!   after each successful extraction cycle and re-injected at construction
//!   as the `__durable_state` config key, so a `mimir stop` / restart resumes
//!   the bounded retry instead of dropping the message.
//!   Persistence is write-through: the supervisor only acknowledges the
//!   ledger as clean after the database write succeeds, so a failed write
//!   keeps the ledger dirty and the next cycle re-persists it. Raw payloads
//!   above [`MAX_PERSISTED_RAW_BYTES`] are retried in-process but not
//!   persisted, and at most [`MAX_PERSISTED_PENDING_PAYLOADS`] pending
//!   payloads are persisted per snapshot (entries beyond the cap still
//!   retry in-process), so a mailbox-wide outage cannot bloat the durable
//!   column with one base64 payload per failing message.
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

/// Cap on the raw RFC 822 payload persisted per pending retry. Prose-only
/// messages are small (the LLM body is capped at 8 KiB), so a message above
/// this cap almost always carries a large attachment; persisting it would
/// bloat the `connectors.durable_state` column and re-serialize it on every
/// backoff cycle. Oversized messages still retry in-process with the full
/// payload, but their payload is not persisted, so a restart drops them
/// (the pre-#262 behaviour) instead of growing the durable state without
/// bound.
pub(crate) const MAX_PERSISTED_RAW_BYTES: usize = 512 * 1024;

/// Cap on the number of pending retries whose raw payload is persisted in
/// one [`ProseRetryLedger::durable_json`] snapshot, so a mailbox-wide LLM
/// outage cannot grow `connectors.durable_state` with one base64 payload per
/// failing message (each payload can be up to
/// [`MAX_PERSISTED_RAW_BYTES`], which base64 expands by ~33 %). Entries
/// beyond the cap still retry in-process with the full payload; only their
/// persisted payload is dropped, so a restart drops them (the pre-#262
/// behaviour) instead of growing the durable state without bound.
pub(crate) const MAX_PERSISTED_PENDING_PAYLOADS: usize = 32;

/// Extraction cycles to wait before retrying after the `attempts`-th
/// failure: exponential backoff (1, 2, 4, …), capped at 8 cycles so a deeply
/// stuck message still yields quickly.
pub(crate) fn backoff_cycles(attempts: u8) -> u8 {
    (1u16 << attempts.saturating_sub(1).min(3)) as u8
}

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

/// A message whose LLM extraction failed and is awaiting a bounded retry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct PendingProse {
    /// IMAP `UIDVALIDITY` of the mailbox the message was fetched from.
    pub uid_validity: u32,
    /// IMAP UID of the message within that `UIDVALIDITY` epoch.
    pub uid: u32,
    /// Raw RFC 822 bytes, base64-encoded so the payload survives a restart
    /// without an IMAP re-fetch (the cursor has already advanced past the
    /// message). `None` when the payload exceeds
    /// [`MAX_PERSISTED_RAW_BYTES`]: the retry still runs in-process, but a
    /// restart cannot resume it.
    #[serde(default)]
    pub raw_b64: Option<String>,
    /// Failed attempts so far (1-based).
    pub attempts: u8,
    /// Last failure reason.
    pub last_error: String,
    /// Extraction cycles still to wait before the next attempt.
    pub skip_cycles: u8,
}

impl PendingProse {
    /// The `UIDVALIDITY`-qualified provenance reference (`{uid_validity}:{uid}`),
    /// matching the `raw_reference` the extraction cascade emits.
    pub fn raw_ref(&self) -> String {
        format!("{}:{}", self.uid_validity, self.uid)
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

/// What the extraction loop may do with a staged message this cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RetryGate {
    /// No ledger entry (or the backoff has elapsed): process normally.
    Attempt,
    /// Awaiting a backoff cycle: re-stage without attempting.
    Backoff,
    /// Retry budget exhausted: drop without processing.
    SkippedTerminal,
}

/// What the extraction loop must do after a failed LLM attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FailureDisposition {
    /// Re-stage the raw email; try again after `skip_cycles` cycles.
    Retry { skip_cycles: u8 },
    /// Budget exhausted: record the terminal failure and drop the message.
    Terminal,
}

/// The durable retry policy state of one Email connector instance.
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
    /// Pending retries keyed by raw reference (`{uid_validity}:{uid}`).
    pending: BTreeMap<String, PendingProse>,
    /// Terminally failed messages (oldest first, capped).
    terminal: Vec<TerminalProseFailure>,
    /// Whether the ledger changed since the last successful persist.
    #[serde(skip)]
    dirty: bool,
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

    /// Decide what the extraction loop may do with the message this cycle.
    ///
    /// A pending retry in its backoff window is decremented (dirty) and
    /// reported as [`RetryGate::Backoff`]; a terminal failure is reported as
    /// [`RetryGate::SkippedTerminal`]; everything else may be attempted.
    pub(crate) fn gate(&mut self, raw_ref: &str) -> RetryGate {
        match self.pending.get_mut(raw_ref) {
            Some(pending) if pending.skip_cycles > 0 => {
                pending.skip_cycles -= 1;
                self.dirty = true;
                RetryGate::Backoff
            }
            Some(_) => RetryGate::Attempt,
            None if terminal_contains(&self.terminal, raw_ref) => RetryGate::SkippedTerminal,
            None => RetryGate::Attempt,
        }
    }

    /// Settle the message: drop any pending retry / terminal record. Called
    /// when the message was successfully read (deterministic facts, LLM
    /// facts, an LLM no-facts verdict, or no backend configured) so a stale
    /// entry can never resurrect a retry.
    pub(crate) fn settle(&mut self, raw_ref: &str) {
        let had_pending = self.pending.remove(raw_ref).is_some();
        let terminal_before = self.terminal.len();
        self.terminal.retain(|t| t.raw_ref() != raw_ref);
        if had_pending || self.terminal.len() != terminal_before {
            self.dirty = true;
        }
    }

    /// Record a failed LLM attempt against the message.
    ///
    /// Bumps the attempt count; when the budget (`max_attempts`) is
    /// exhausted the message moves to the terminal ledger and
    /// [`FailureDisposition::Terminal`] is returned, otherwise the message
    /// is re-staged after an exponential backoff
    /// ([`FailureDisposition::Retry`]).
    pub(crate) fn record_failure(
        &mut self,
        raw_ref: &str,
        uid_validity: u32,
        uid: u32,
        raw: &[u8],
        max_attempts: u8,
        error: String,
    ) -> FailureDisposition {
        self.dirty = true;
        let attempts = self
            .pending
            .get(raw_ref)
            .map(|pending| pending.attempts.saturating_add(1))
            .unwrap_or(1);
        if attempts >= max_attempts {
            self.pending.remove(raw_ref);
            self.terminal.retain(|t| t.raw_ref() != raw_ref);
            self.terminal.push(TerminalProseFailure {
                uid_validity,
                uid,
                attempts,
                last_error: error,
            });
            self.trim_terminal();
            return FailureDisposition::Terminal;
        }
        let raw_b64 = if raw.len() <= MAX_PERSISTED_RAW_BYTES {
            Some(STANDARD.encode(raw))
        } else {
            warn!(
                raw_ref,
                size = raw.len(),
                "raw email exceeds the persisted-retry size cap; the retry will not survive a restart"
            );
            None
        };
        let skip_cycles = backoff_cycles(attempts);
        self.pending.insert(
            raw_ref.to_string(),
            PendingProse {
                uid_validity,
                uid,
                raw_b64,
                attempts,
                last_error: error,
                skip_cycles,
            },
        );
        FailureDisposition::Retry { skip_cycles }
    }

    /// Serialise the ledger when it changed since the last successful
    /// persist. `None` means nothing new to persist. Unlike a "take", this
    /// does **not** clear the dirty flag: the supervisor calls
    /// [`mark_persisted`](Self::mark_persisted) only after the database
    /// write succeeds, so a failed write leaves the ledger dirty and the
    /// next cycle re-persists it (a durable state that is only read once
    /// would be silently lost if the write failed).
    pub(crate) fn durable_json(&self) -> Option<String> {
        if !self.dirty {
            return None;
        }
        // Serialise a snapshot with the persisted-payload cap applied, so
        // the in-memory ledger keeps the full payload for every pending
        // retry while the durable column stays bounded.
        let mut snapshot = self.clone();
        let mut kept = 0usize;
        for pending in snapshot.pending.values_mut() {
            if pending.raw_b64.is_some() {
                kept += 1;
                if kept > MAX_PERSISTED_PENDING_PAYLOADS {
                    pending.raw_b64 = None;
                }
            }
        }
        match serde_json::to_string(&snapshot) {
            Ok(json) => Some(json),
            Err(error) => {
                warn!(%error, "failed to serialize prose-retry ledger; durable state not persisted");
                None
            }
        }
    }

    /// Mark the ledger clean after the supervisor persisted
    /// [`durable_json`](Self::durable_json) successfully.
    pub(crate) fn mark_persisted(&mut self) {
        self.dirty = false;
    }

    /// Pending retries, for re-staging at construction.
    pub(crate) fn pending(&self) -> impl Iterator<Item = &PendingProse> {
        self.pending.values()
    }

    /// Number of terminally failed messages (drives `Degraded` health).
    pub(crate) fn terminal_count(&self) -> usize {
        self.terminal.len()
    }

    /// Whether the ledger holds no pending retries and no terminal failures.
    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.pending.is_empty() && self.terminal.is_empty()
    }

    /// Drop every record without marking the ledger dirty (used by `forget`,
    /// where the row — and its durable state — is deleted anyway).
    pub(crate) fn clear(&mut self) {
        self.pending.clear();
        self.terminal.clear();
        self.dirty = false;
    }

    fn trim_terminal(&mut self) {
        if self.terminal.len() > MAX_TERMINAL_FAILURES {
            let excess = self.terminal.len() - MAX_TERMINAL_FAILURES;
            self.terminal.drain(..excess);
        }
    }

    /// Normalise a restored ledger: a raw reference may be pending or
    /// terminal, never both (the terminal record is the stricter state, so
    /// any shadowed pending entry is dropped), and the terminal list is
    /// re-capped. Marks the ledger dirty when something was removed so the
    /// correction is persisted.
    fn sanitize(&mut self) {
        let mut changed = false;
        for terminal in &self.terminal {
            changed |= self.pending.remove(&terminal.raw_ref()).is_some();
        }
        let terminal_len = self.terminal.len();
        self.trim_terminal();
        changed |= self.terminal.len() != terminal_len;
        if changed {
            self.dirty = true;
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
    fn backoff_schedule_is_exponential_and_capped() {
        assert_eq!(backoff_cycles(0), 1);
        assert_eq!(backoff_cycles(1), 1);
        assert_eq!(backoff_cycles(2), 2);
        assert_eq!(backoff_cycles(3), 4);
        assert_eq!(backoff_cycles(4), 8);
        assert_eq!(backoff_cycles(10), 8);
    }

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
    fn gate_allows_messages_without_a_ledger_entry() {
        let mut ledger = ProseRetryLedger::default();
        assert_eq!(ledger.gate("17:1"), RetryGate::Attempt);
        assert!(!ledger.dirty, "an empty gate must not dirty the ledger");
    }

    #[test]
    fn first_failure_stages_a_bounded_retry_with_backoff() {
        let mut ledger = ProseRetryLedger::default();
        let disposition =
            ledger.record_failure("17:1", 17, 1, b"raw bytes", 3, "queue full".into());
        assert_eq!(disposition, FailureDisposition::Retry { skip_cycles: 1 });
        assert_eq!(ledger.gate("17:1"), RetryGate::Backoff);
        assert!(ledger.dirty, "backoff decrement marks the ledger dirty");
        assert_eq!(ledger.gate("17:1"), RetryGate::Attempt);
        let pending = ledger.pending.get("17:1").expect("pending entry");
        assert_eq!(pending.attempts, 1);
        assert_eq!(pending.last_error, "queue full");
        assert_eq!(pending.raw().as_deref(), Some(b"raw bytes".as_slice()));
    }

    #[test]
    fn retries_are_bounded_and_then_terminal() {
        let mut ledger = ProseRetryLedger::default();
        assert_eq!(
            ledger.record_failure("17:1", 17, 1, b"raw", 3, "e1".into()),
            FailureDisposition::Retry { skip_cycles: 1 }
        );
        ledger.gate("17:1"); // backoff cycle 1
        assert_eq!(
            ledger.record_failure("17:1", 17, 1, b"raw", 3, "e2".into()),
            FailureDisposition::Retry { skip_cycles: 2 }
        );
        ledger.gate("17:1"); // backoff cycle 1
        ledger.gate("17:1"); // backoff cycle 2
        assert_eq!(
            ledger.record_failure("17:1", 17, 1, b"raw", 3, "e3".into()),
            FailureDisposition::Terminal
        );
        assert!(
            ledger.pending.is_empty(),
            "terminal failure drops the pending entry"
        );
        assert_eq!(ledger.terminal.len(), 1);
        assert_eq!(ledger.terminal[0].attempts, 3);
        assert_eq!(ledger.terminal[0].last_error, "e3");
        assert_eq!(ledger.gate("17:1"), RetryGate::SkippedTerminal);
    }

    #[test]
    fn max_attempts_one_fails_terminal_immediately() {
        let mut ledger = ProseRetryLedger::default();
        let disposition = ledger.record_failure("17:1", 17, 1, b"raw", 1, "boom".into());
        assert_eq!(disposition, FailureDisposition::Terminal);
        assert!(ledger.pending.is_empty());
        assert_eq!(ledger.terminal.len(), 1);
        assert_eq!(ledger.gate("17:1"), RetryGate::SkippedTerminal);
    }

    #[test]
    fn settle_clears_pending_and_terminal_entries() {
        let mut ledger = ProseRetryLedger::default();
        ledger.record_failure("17:1", 17, 1, b"raw", 1, "boom".into());
        assert_eq!(ledger.gate("17:1"), RetryGate::SkippedTerminal);
        ledger.settle("17:1");
        assert!(ledger.is_empty());
        assert_eq!(ledger.gate("17:1"), RetryGate::Attempt);
    }

    #[test]
    fn durable_round_trip_preserves_pending_and_terminal() {
        let mut ledger = ProseRetryLedger::default();
        ledger.record_failure("17:1", 17, 1, b"raw bytes", 3, "e1".into());
        ledger.record_failure("99:2", 99, 2, b"other", 1, "fatal".into());
        let json = ledger.durable_json().expect("dirty ledger serializes");
        let mut restored = ProseRetryLedger::from_json(&json);
        assert_eq!(restored.pending.len(), 1);
        let pending = restored.pending.get("17:1").expect("pending entry");
        assert_eq!(pending.attempts, 1);
        assert_eq!(pending.raw().as_deref(), Some(b"raw bytes".as_slice()));
        assert_eq!(restored.terminal.len(), 1);
        assert_eq!(restored.terminal[0].raw_ref(), "99:2");
        assert!(!restored.dirty, "restored ledger starts clean");
        assert_eq!(restored.gate("17:1"), RetryGate::Backoff);
        assert_eq!(restored.gate("17:1"), RetryGate::Attempt);
    }

    #[test]
    fn durable_json_is_dirty_tracking_until_persisted() {
        let mut ledger = ProseRetryLedger::default();
        assert_eq!(ledger.durable_json(), None, "clean ledger persists nothing");
        ledger.record_failure("17:1", 17, 1, b"raw", 3, "e".into());
        // Reading the durable state must not clear the dirty flag: the
        // supervisor acknowledges the persist only after the database write
        // succeeds, so a failed write leaves the state pending a retry.
        assert!(ledger.durable_json().is_some(), "dirty ledger persists");
        assert!(
            ledger.durable_json().is_some(),
            "an unacknowledged read must not mark the ledger clean"
        );
        ledger.mark_persisted();
        assert_eq!(
            ledger.durable_json(),
            None,
            "only an acknowledged persist clears the ledger"
        );
    }

    #[test]
    fn from_json_falls_back_on_corrupt_payload() {
        let restored = ProseRetryLedger::from_json("not json");
        assert!(restored.is_empty());
    }

    #[test]
    fn from_json_sanitizes_overlapping_pending_and_terminal() {
        let mut ledger = ProseRetryLedger::default();
        ledger.record_failure("17:1", 17, 1, b"raw", 1, "boom".into());
        let mut value: serde_json::Value =
            serde_json::from_str(&ledger.durable_json().expect("dirty")).unwrap();
        // Hand-crafted overlap: the same raw reference pending and terminal.
        value["pending"]["17:1"] = serde_json::json!({
            "uid_validity": 17,
            "uid": 1,
            "raw_b64": "cmF3",
            "attempts": 1,
            "last_error": "e",
            "skip_cycles": 1
        });
        let mut restored = ProseRetryLedger::from_json(&value.to_string());
        assert!(
            restored.pending.is_empty(),
            "a terminal record must shadow a pending entry for the same reference"
        );
        assert_eq!(restored.gate("17:1"), RetryGate::SkippedTerminal);
        assert!(
            restored.dirty,
            "sanitising a restored ledger must mark it for re-persist"
        );
    }

    #[test]
    fn oversized_raw_is_not_persisted_but_still_retries_in_process() {
        let mut ledger = ProseRetryLedger::default();
        let big = vec![b'x'; MAX_PERSISTED_RAW_BYTES + 1];
        let disposition = ledger.record_failure("17:1", 17, 1, &big, 3, "e".into());
        assert_eq!(disposition, FailureDisposition::Retry { skip_cycles: 1 });
        let pending = ledger.pending.get("17:1").expect("pending entry");
        assert!(
            pending.raw_b64.is_none(),
            "oversized payloads must not be persisted"
        );
        assert_eq!(pending.raw(), None);
        assert_eq!(ledger.gate("17:1"), RetryGate::Backoff);
        assert_eq!(ledger.gate("17:1"), RetryGate::Attempt);
        let json = ledger.durable_json().expect("dirty ledger persists");
        let restored = ProseRetryLedger::from_json(&json);
        assert!(
            restored
                .pending
                .get("17:1")
                .expect("pending entry")
                .raw_b64
                .is_none(),
            "round-trip keeps the payload absent"
        );
    }

    #[test]
    fn persisted_pending_payloads_are_capped_but_in_process_retry_keeps_them() {
        let mut ledger = ProseRetryLedger::default();
        let total = MAX_PERSISTED_PENDING_PAYLOADS + 5;
        for i in 0..total {
            ledger.record_failure(
                &format!("17:{i}"),
                17,
                i as u32,
                format!("raw-{i}").as_bytes(),
                3,
                format!("e{i}"),
            );
        }
        // The in-memory ledger keeps every payload for in-process retries.
        assert_eq!(ledger.pending.len(), total);
        assert_eq!(
            ledger
                .pending
                .values()
                .filter(|p| p.raw_b64.is_some())
                .count(),
            total,
            "the cap must not shed in-memory payloads"
        );
        // The persisted snapshot sheds payloads beyond the cap, but keeps
        // every pending entry (payload-less entries still retry in-process
        // and are only dropped by a restart).
        let json = ledger.durable_json().expect("dirty ledger persists");
        let restored = ProseRetryLedger::from_json(&json);
        assert_eq!(restored.pending.len(), total);
        assert_eq!(
            restored
                .pending
                .values()
                .filter(|p| p.raw_b64.is_some())
                .count(),
            MAX_PERSISTED_PENDING_PAYLOADS,
            "only the cap's worth of payloads may be persisted"
        );
    }

    #[test]
    fn terminal_records_are_capped_oldest_first() {
        let mut ledger = ProseRetryLedger::default();
        for i in 0..(MAX_TERMINAL_FAILURES + 5) {
            ledger.record_failure(&format!("17:{i}"), 17, i as u32, b"raw", 1, format!("e{i}"));
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
    fn clear_resets_everything_without_dirtying() {
        let mut ledger = ProseRetryLedger::default();
        ledger.record_failure("17:1", 17, 1, b"raw", 3, "e".into());
        ledger.clear();
        assert!(ledger.is_empty());
        assert!(!ledger.dirty);
        assert_eq!(ledger.durable_json(), None);
    }
}
