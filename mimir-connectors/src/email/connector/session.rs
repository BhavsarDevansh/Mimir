//! IMAP session lifecycle: open, sync, and capability probing.

use std::collections::HashSet;
use std::sync::atomic::Ordering;
use std::time::Duration;

use chrono::Utc;
use tracing::{debug, warn};

use crate::connector::{ConnectorError, SyncOptions, SyncOutcome};
use crate::email::config::{DEFAULT_IMAP_PORT, DEFAULT_MAILBOX, EmailSyncMode, encode_cursor};
use crate::email::connector::EmailConnector;
use crate::email::imap;
use crate::email::imap::{FetchResult, ImapSession, connect_tls, imap_login};

/// Consecutive [`imap::IdleResult::ConnectionLost`] outcomes tolerated
/// before the cycle is failed. A provider that keeps dropping the IDLE
/// connection (an inactivity limit shorter than the configured timeout, or
/// a flaky path) would otherwise loop straight back into a fresh TLS
/// reconnect with no delay — every dropped-IDLE cycle reports success, so
/// the supervisor's push loop never backs off. Escalating to a cycle error
/// hands the loop to the supervisor's exponential backoff instead.
const MAX_CONSECUTIVE_CONNECTION_LOST: u32 = 3;

impl EmailConnector {
    pub(crate) fn port(&self) -> u16 {
        self.config.port.unwrap_or(DEFAULT_IMAP_PORT)
    }
    pub(crate) fn mailbox(&self) -> &str {
        self.config.mailbox.as_deref().unwrap_or(DEFAULT_MAILBOX)
    }
    fn idle_timeout(&self) -> Duration {
        Duration::from_secs(self.config.idle_timeout_secs)
    }
    /// Record one IDLE `ConnectionLost` outcome and report whether the
    /// consecutive run now exceeds the escalation threshold. The counter is
    /// reset only by a normally-completed IDLE wait ([`imap::IdleResult::NewData`]
    /// or [`imap::IdleResult::Timeout`]) — the re-sync cycles between two
    /// drops never exercise IDLE, so they must not reset it.
    fn record_connection_lost_escalated(&self) -> bool {
        self.consecutive_connection_lost
            .fetch_add(1, Ordering::SeqCst)
            + 1
            >= MAX_CONSECUTIVE_CONNECTION_LOST
    }
    /// Record one dropped-IDLE outcome and fail the cycle once the
    /// consecutive run exceeds the tolerated threshold, so the
    /// supervisor's exponential backoff breaks a rapid reconnect loop.
    fn record_idle_connection_lost(&self) -> Result<(), ConnectorError> {
        if self.record_connection_lost_escalated() {
            warn!("IDLE connection lost repeatedly; failing the cycle so the supervisor backs off");
            return Err(ConnectorError::Network(
                "IMAP IDLE connection lost repeatedly; backing off before reconnecting".into(),
            ));
        }
        Ok(())
    }
    pub(crate) fn connect_timeout(&self) -> Duration {
        Duration::from_secs(self.config.connect_timeout_secs)
    }
    pub(crate) fn handshake_timeout(&self) -> Duration {
        Duration::from_secs(self.config.handshake_timeout_secs)
    }
    pub(crate) fn read_timeout(&self) -> Duration {
        Duration::from_secs(self.config.read_timeout_secs)
    }

    /// Decide whether this cycle uses IDLE (Push) or polling (Polling).
    /// `Ok(true)` → IDLE. Honours the explicit config mode, falling back to
    /// the cached capability for `auto`. `Idle` mode errors if the server is
    /// known to lack `IDLE`, matching the documented contract. Synchronous
    /// (a `std::sync::Mutex` guard never held across an `await`).
    pub(super) fn use_idle(&self) -> Result<bool, ConnectorError> {
        match self.config.mode {
            // Forced IDLE: error if the capability probe confirmed the server
            // does not advertise `IDLE`. An unprobed (`None`) cache lets the
            // IDLE attempt proceed; the server's BAD response surfaces the
            // mismatch on the next cycle.
            EmailSyncMode::Idle => match *self.supports_idle.lock().unwrap() {
                Some(false) => Err(ConnectorError::Config(
                    "idle mode requested but the server does not advertise IDLE".into(),
                )),
                _ => Ok(true),
            },
            EmailSyncMode::Poll => Ok(false),
            // `Auto` defaults to Push when unprobed, matching `mode()` (which
            // reports `ConnectorMode::Push` for `None`); otherwise follows the
            // cached capability so `mode()` and `use_idle()` never disagree
            // (a mismatch would let the supervisor's push loop busy-spin).
            EmailSyncMode::Auto => Ok(self.supports_idle.lock().unwrap().unwrap_or(true)),
        }
    }

    /// Open an authenticated session to the configured IMAP server, resolving
    /// credentials (and persisting any OAuth refresh) first.
    pub(super) async fn open_session(
        &self,
    ) -> Result<ImapSession<imap::TlsStream>, ConnectorError> {
        let (auth, refreshed) = self.resolve_credentials().await?;
        if let Some(b) = refreshed {
            self.persist_refreshed(&b).await?;
        }
        let (stream, deadline) = connect_tls(
            &self.config.host,
            self.port(),
            self.connect_timeout(),
            self.handshake_timeout(),
        )
        .await?;
        imap_login(stream, auth, deadline, self.read_timeout()).await
    }

    /// Run one sync cycle against an already-authenticated session. Generic
    /// over the stream so tests drive it against a fake server. Selects the
    /// mailbox, validates the UIDVALIDITY cursor, optionally blocks on IDLE
    /// (Push), then incrementally `UID FETCH`es and stages new messages.
    pub(super) async fn run_sync<S: imap::ImapStream>(
        &self,
        mut session: ImapSession<S>,
        options: SyncOptions,
    ) -> Result<SyncOutcome, ConnectorError> {
        // Issue #332: a previous cycle reported a moved cursor that the
        // supervisor has not yet confirmed via `on_cycle_succeeded` (the
        // cycle failed after `sync`). The IDLE notification for that window
        // will not re-fire, so skip the IDLE wait and re-fetch from the last
        // confirmed cursor immediately instead of blocking until the next
        // push.
        let idle = self.use_idle()? && !self.resync_pending.load(Ordering::SeqCst);
        let info = session.examine(self.mailbox()).await?;
        let uid_validity = info.uid_validity;

        // Validate the persisted cursor against the current UIDVALIDITY:
        // a mismatch (mailbox recreated) invalidates all prior UIDs → full.
        let cursor = *self.last_uid.lock().await;
        let last_uid = match (cursor, options.full) {
            (_, true) => None,
            (Some((v, u)), false) if v == uid_validity => Some(u),
            _ => None,
        };
        // Issue #397: an `initial_backfill: false` connector starts from
        // "now" — the first cycle (no cursor) seeds the cursor to the
        // mailbox's current `UIDNEXT` instead of full-fetching, so only mail
        // arriving after setup is ingested. `full` overrides: an explicit
        // full sync still fetches everything. The seed applies only to a
        // true first sync (`cursor.is_none()`), never to a persisted cursor
        // whose UIDVALIDITY no longer matches: a recreated mailbox invalidates
        // every prior UID, so it must full re-sync even when the user chose
        // "only new content". A first sync that cannot anchor on a `UIDNEXT`
        // fails instead of silently full-fetching content the user opted out
        // of.
        let seed = if cursor.is_none() && !self.config.initial_backfill && !options.full {
            match info.uid_next {
                Some(next) => Some(next.saturating_sub(1)),
                None => {
                    return Err(ConnectorError::Parse(
                        "server did not report UIDNEXT; cannot seed the 'only new content' cursor"
                            .into(),
                    ));
                }
            }
        } else {
            None
        };
        let mut last_uid = match (last_uid, seed) {
            (None, Some(seeded)) => Some(seeded),
            (last_uid, _) => last_uid,
        };
        if cursor.is_some() && matches!(cursor, Some((v, _)) if v != uid_validity) && seed.is_none()
        {
            warn!(
                uid_validity,
                "IMAP UIDVALIDITY changed; performing full re-sync"
            );
        }

        // Issue #397: a fresh push-mode connector (no cursor) must import the
        // existing mailbox before blocking on IDLE — otherwise the first
        // cycle connects, EXAMINEs, and waits for the first new message while
        // the current inbox is never fetched. The backfill runs once, then
        // the cycle blocks on IDLE as usual.
        let mut backfilled: u32 = 0;
        let mut session = if idle && last_uid.is_none() {
            let FetchResult { messages, max_uid } = session.fetch_since(None, uid_validity).await?;
            backfilled = u32::try_from(messages.len()).unwrap_or(u32::MAX);
            self.stage_messages(messages).await;
            match session.idle_wait(self.idle_timeout()).await? {
                // NewData: fetch the pushed mail. Timeout: fetch anyway — a
                // server that never pushes (or a notification lost in the
                // timeout race) must not strand mail that arrived during
                // the window. Both continue to the incremental fetch below,
                // from the backfilled UID so the whole mailbox is never
                // re-fetched.
                imap::IdleResult::NewData(sess) | imap::IdleResult::Timeout(sess) => {
                    self.consecutive_connection_lost.store(0, Ordering::SeqCst);
                    last_uid = Some(max_uid);
                    sess
                }
                // The server dropped the connection during IDLE (e.g. a
                // provider inactivity close). The backfill already staged
                // the existing mailbox: report its cursor so the cycle
                // succeeds and the staged mail is extracted instead of being
                // lost to a cycle failure, and mark a re-sync pending so the
                // next cycle re-fetches the window immediately instead of
                // blocking on IDLE (the push for that window will not
                // re-fire).
                imap::IdleResult::ConnectionLost => {
                    debug!(
                        "IDLE connection dropped mid-window (provider inactivity close); \
                         reporting backfill progress and re-syncing next cycle"
                    );
                    self.resync_pending.store(true, Ordering::SeqCst);
                    self.record_idle_connection_lost()?;
                    return Ok(SyncOutcome {
                        fetched: backfilled,
                        new_cursor: Some(encode_cursor(uid_validity, max_uid)),
                        fetched_at: Utc::now(),
                    });
                }
            }
        } else if idle {
            match session.idle_wait(self.idle_timeout()).await? {
                // NewData / Timeout: continue to the incremental fetch below
                // (see the backfill arm for why a timeout still fetches).
                imap::IdleResult::NewData(sess) | imap::IdleResult::Timeout(sess) => {
                    self.consecutive_connection_lost.store(0, Ordering::SeqCst);
                    sess
                }
                imap::IdleResult::ConnectionLost => {
                    debug!(
                        "IDLE connection dropped mid-window (provider inactivity close); \
                         re-syncing next cycle"
                    );
                    self.resync_pending.store(true, Ordering::SeqCst);
                    self.record_idle_connection_lost()?;
                    return Ok(SyncOutcome {
                        fetched: 0,
                        // Persist the "start from now" seed even when no mail
                        // arrived, so the no-backfill choice survives a
                        // restart.
                        new_cursor: seed.map(|s| encode_cursor(uid_validity, s)),
                        fetched_at: Utc::now(),
                    });
                }
            }
        } else {
            session
        };

        let FetchResult { messages, max_uid } = session.fetch_since(last_uid, uid_validity).await?;
        session.logout().await;

        let fetched = u32::try_from(messages.len())
            .unwrap_or(u32::MAX)
            .saturating_add(backfilled);
        self.stage_messages(messages).await;

        // Report the cursor: persist on a full/first sync or when new mail
        // arrived; leave it unchanged when an incremental cycle fetched
        // nothing (the supervisor skips a no-op cursor write). The in-memory
        // marker is deliberately NOT advanced here — the supervisor persists
        // the reported cursor and hands it back via
        // `Connector::on_cycle_succeeded` only after a fully successful
        // cycle, so a cycle that fails after `sync` re-syncs from the last
        // confirmed cursor on the next in-process cycle instead of skipping
        // the failed window (issue #332, mirroring #314).
        let new_cursor = match (last_uid, max_uid) {
            (None, _) => Some(encode_cursor(uid_validity, max_uid)),
            (Some(prev), max) if max > prev => Some(encode_cursor(uid_validity, max)),
            // Seeded first sync: persist the seed even when nothing arrived.
            _ if seed.is_some() => Some(encode_cursor(uid_validity, last_uid.unwrap_or(0))),
            // Push backfill: persist the backfill cursor even when the IDLE
            // push that woke the cycle carried no fetchable mail — otherwise
            // the next cycle sees no cursor and re-backfills the whole
            // mailbox until the first real new mail arrives (issue #397).
            _ if backfilled > 0 => Some(encode_cursor(uid_validity, max_uid)),
            _ => None,
        };
        // Track whether a re-fetch is pending: the next cycle then skips the
        // IDLE wait and re-fetches immediately. Set when a cursor moved
        // (issue #332: a cycle that fails after `sync` must re-fetch the
        // failed window because the IDLE notification will not re-fire) or
        // when the IDLE connection was dropped mid-window (the push for that
        // window is lost). Cleared by the next successful fetch — the
        // `on_cycle_succeeded` hook no longer clears it, so a
        // `ConnectionLost` cycle's re-fetch actually runs before IDLE
        // resumes.
        self.resync_pending
            .store(new_cursor.is_some(), Ordering::SeqCst);

        debug!(
            fetched,
            uid_validity,
            reported_last_uid = max_uid,
            idle,
            "email sync cycle complete"
        );

        Ok(SyncOutcome {
            fetched,
            new_cursor,
            fetched_at: Utc::now(),
        })
    }

    /// Stage fetched messages into the cycle buffer, deduplicating by the
    /// `(uid_validity, uid)` identity (issue #332 mirror: a failed cycle's
    /// re-fetch, a `--full` re-sync, or a push backfill (issue #397) can
    /// stage the same message twice, and each duplicate would otherwise
    /// double-insert facts).
    async fn stage_messages(&self, messages: Vec<imap::RawEmail>) {
        let mut buffer = self.buffer.lock().await;
        let mut seen: HashSet<(u32, u32)> =
            buffer.iter().map(|m| (m.uid_validity, m.uid)).collect();
        for mail in messages {
            if seen.insert((mail.uid_validity, mail.uid)) {
                buffer.push(mail);
            }
        }
    }

    /// Shared probe used by both [`Connector::authenticate`] and
    /// [`Connector::health`]: resolve credentials (refreshing OAuth),
    /// connect, log in, probe `CAPABILITY` (caching IDLE support), and log
    /// out. Returns the cached `IDLE` capability. Callers map the
    /// [`ConnectorError`] onto their respective lifecycle enums.
    /// The capability is also recorded in the durable state (issue #397
    /// review) so a fresh instance — a daemon restart or a
    /// `resolved_mode` construction — resolves `Auto` mode without a live
    /// probe.
    pub(super) async fn probe_capability(&self) -> Result<bool, ConnectorError> {
        let (auth, refreshed) = self.resolve_credentials().await?;
        if let Some(b) = refreshed {
            self.persist_refreshed(&b).await?;
        }
        let (stream, deadline) = connect_tls(
            &self.config.host,
            self.port(),
            self.connect_timeout(),
            self.handshake_timeout(),
        )
        .await?;
        let mut session = imap_login(stream, auth, deadline, self.read_timeout()).await?;
        let supports = match session.supports_idle().await {
            Ok(supports) => supports,
            Err(e) => {
                session.logout().await;
                return Err(e);
            }
        };
        session.logout().await;
        *self.supports_idle.lock().unwrap() = Some(supports);
        self.prose_retry.lock().unwrap().set_supports_idle(supports);
        Ok(supports)
    }
}
