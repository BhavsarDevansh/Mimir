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
    /// message).
    pub raw_b64: String,
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
    /// corrupt (the item is then settled and dropped).
    pub fn raw(&self) -> Option<Vec<u8>> {
        STANDARD.decode(&self.raw_b64).ok()
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
/// [`take_durable`](Self::take_durable)); `dirty` tracks changes since the
/// last persist and is never serialised.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub(crate) struct ProseRetryLedger {
    /// Pending retries keyed by raw reference (`{uid_validity}:{uid}`).
    pending: BTreeMap<String, PendingProse>,
    /// Terminally failed messages (oldest first, capped).
    terminal: Vec<TerminalProseFailure>,
    /// Whether the ledger changed since the last [`take_durable`](Self::take_durable).
    #[serde(skip)]
    dirty: bool,
}

impl ProseRetryLedger {
    /// Parse a persisted ledger, falling back to an empty one (with a warn)
    /// when the stored JSON is corrupt.
    pub(crate) fn from_json(json: &str) -> Self {
        match serde_json::from_str(json) {
            Ok(ledger) => ledger,
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
            None if self.terminal.iter().any(|t| t.raw_ref() == raw_ref) => {
                RetryGate::SkippedTerminal
            }
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
            self.terminal.push(TerminalProseFailure {
                uid_validity,
                uid,
                attempts,
                last_error: error,
            });
            self.trim_terminal();
            return FailureDisposition::Terminal;
        }
        let skip_cycles = backoff_cycles(attempts);
        self.pending.insert(
            raw_ref.to_string(),
            PendingProse {
                uid_validity,
                uid,
                raw_b64: STANDARD.encode(raw),
                attempts,
                last_error: error,
                skip_cycles,
            },
        );
        FailureDisposition::Retry { skip_cycles }
    }

    /// Serialise the ledger when it changed since the last persist, marking
    /// it clean. `None` means nothing new to persist.
    pub(crate) fn take_durable(&mut self) -> Option<String> {
        if !self.dirty {
            return None;
        }
        self.dirty = false;
        match serde_json::to_string(self) {
            Ok(json) => Some(json),
            Err(error) => {
                self.dirty = true;
                warn!(%error, "failed to serialize prose-retry ledger; durable state not persisted");
                None
            }
        }
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
        assert_eq!(STANDARD.decode(&pending.raw_b64).unwrap(), b"raw bytes");
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
        let json = ledger.take_durable().expect("dirty ledger serializes");
        let mut restored = ProseRetryLedger::from_json(&json);
        assert_eq!(restored.pending.len(), 1);
        let pending = restored.pending.get("17:1").expect("pending entry");
        assert_eq!(pending.attempts, 1);
        assert_eq!(STANDARD.decode(&pending.raw_b64).unwrap(), b"raw bytes");
        assert_eq!(restored.terminal.len(), 1);
        assert_eq!(restored.terminal[0].raw_ref(), "99:2");
        assert!(!restored.dirty, "restored ledger starts clean");
        assert_eq!(restored.gate("17:1"), RetryGate::Backoff);
        assert_eq!(restored.gate("17:1"), RetryGate::Attempt);
    }

    #[test]
    fn take_durable_is_dirty_tracking() {
        let mut ledger = ProseRetryLedger::default();
        assert_eq!(ledger.take_durable(), None, "clean ledger persists nothing");
        ledger.record_failure("17:1", 17, 1, b"raw", 3, "e".into());
        assert!(ledger.take_durable().is_some(), "dirty ledger persists");
        assert_eq!(ledger.take_durable(), None, "second call persists nothing");
    }

    #[test]
    fn from_json_falls_back_on_corrupt_payload() {
        let restored = ProseRetryLedger::from_json("not json");
        assert!(restored.is_empty());
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
        assert_eq!(ledger.take_durable(), None);
    }
}
