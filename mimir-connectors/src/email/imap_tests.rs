use super::*;

use crate::email::config::config_tests::app_config;
use crate::email::imap::{ImapAuth, ImapSession, imap_login};
use async_imap::Client;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use super::extract_tests::invite_email;

/// Generous greeting budget for fake-server tests: the fake greets
/// immediately, so this only fires if the login path regresses into a stall.
const TEST_GREETING_BUDGET: Duration = Duration::from_secs(5);

/// Configuration for the fake IMAP server.
struct FakeCfg {
    uid_validity: u32,
    supports_idle: bool,
    /// (uid, body) pairs the server exposes via `UID FETCH`.
    messages: Vec<(u32, Vec<u8>)>,
    /// `EXISTS` count to push during IDLE (signals new mail). `None` → IDLE
    /// times out with no push (connector returns fetched:0).
    idle_push_exists: Option<u32>,
    /// Messages appended to the mailbox when the IDLE push fires, modelling
    /// mail that arrives while the connector is blocked on IDLE. `empty` →
    /// no new messages.
    idle_push_messages: Vec<(u32, Vec<u8>)>,
    /// Second UIDVALIDITY returned on a *second* `SELECT` (UIDVALIDITY
    /// reset test). `None` → always returns `uid_validity`.
    second_uid_validity: Option<u32>,
    /// Omit the `UIDVALIDITY` response code on SELECT/EXAMINE to exercise
    /// the missing-UIDVALIDITY error path. `false` by default.
    omit_uid_validity: bool,
    /// Omit the `UIDNEXT` response code on SELECT/EXAMINE to exercise the
    /// missing-UIDNEXT first-sync seed error path. `false` by default.
    omit_uid_next: bool,
}

impl Default for FakeCfg {
    fn default() -> Self {
        Self {
            uid_validity: 17,
            supports_idle: true,
            messages: Vec::new(),
            idle_push_exists: None,
            idle_push_messages: Vec::new(),
            second_uid_validity: None,
            omit_uid_validity: false,
            omit_uid_next: false,
        }
    }
}

/// Drive a fake IMAP server over `stream`. Handles exactly the verbs the
/// connector issues (greeting, LOGIN/AUTHENTICATE, SELECT, UID FETCH, IDLE,
/// LOGOUT). Captures the decoded XOAUTH2 SASL response into `capture` when
/// supplied, and every `UID FETCH` range into `fetch_ranges` when supplied.
async fn run_fake(
    stream: tokio::io::DuplexStream,
    mut cfg: FakeCfg,
    capture: Option<Arc<Mutex<Vec<u8>>>>,
    fetch_ranges: Option<Arc<Mutex<Vec<String>>>>,
    select_count: Arc<Mutex<u32>>,
) {
    use base64::Engine as _;
    let (read, mut writer) = tokio::io::split(stream);
    let mut reader = BufReader::new(read);
    writer
        .write_all(b"* OK fake IMAP ready\r\n")
        .await
        .expect("greeting");
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).await.unwrap() == 0 {
            break;
        }
        let line = line.trim_end_matches(['\r', '\n']).to_string();
        let mut parts = line.split_whitespace();
        let tag = parts.next().unwrap_or("").to_string();
        let verb = parts.next().unwrap_or("").to_ascii_uppercase();
        match verb.as_str() {
            "CAPABILITY" => {
                let cap = if cfg.supports_idle {
                    "IMAP4rev1 IDLE"
                } else {
                    "IMAP4rev1"
                };
                writer
                    .write_all(format!("* CAPABILITY {cap}\r\n{tag} OK CAPABILITY\r\n").as_bytes())
                    .await
                    .unwrap();
            }
            "LOGIN" => {
                writer
                    .write_all(format!("{tag} OK LOGIN completed\r\n").as_bytes())
                    .await
                    .unwrap();
            }
            "AUTHENTICATE" => {
                // XOAUTH2: send an empty continuation challenge.
                writer.write_all(b"+ \r\n").await.unwrap();
                let mut resp = String::new();
                reader.read_line(&mut resp).await.unwrap();
                if let Some(cap) = &capture {
                    let trimmed = resp.trim_end_matches(['\r', '\n']);
                    if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(trimmed) {
                        *cap.lock().unwrap() = bytes;
                    }
                }
                writer
                    .write_all(format!("{tag} OK AUTHENTICATE completed\r\n").as_bytes())
                    .await
                    .unwrap();
            }
            "SELECT" | "EXAMINE" => {
                // Compute UIDVALIDITY (per-SELECT for the reset test) and
                // drop the guard before awaiting so the future stays Send.
                let uv = {
                    let mut n = select_count.lock().unwrap();
                    *n += 1;
                    if *n >= 2 {
                        cfg.second_uid_validity.unwrap_or(cfg.uid_validity)
                    } else {
                        cfg.uid_validity
                    }
                };
                let exists = cfg.messages.len() as u32;
                let next = cfg.messages.iter().map(|(u, _)| *u).max().unwrap_or(0) + 1;
                let uidvalidity_line = if cfg.omit_uid_validity {
                    String::new()
                } else {
                    format!("* OK [UIDVALIDITY {uv}]\r\n")
                };
                let uidnext_line = if cfg.omit_uid_next {
                    String::new()
                } else {
                    format!("* OK [UIDNEXT {next}]\r\n")
                };
                writer
                        .write_all(
                            format!(
                                "* FLAGS (\\Seen)\r\n* {exists} EXISTS\r\n{uidvalidity_line}{uidnext_line}{tag} OK [READ-WRITE] SELECT completed\r\n",
                            )
                            .as_bytes(),
                        )
                        .await
                        .unwrap();
            }
            "UID" => {
                // `UID FETCH <range> (UID INTERNALDATE BODY.PEEK[])`
                let sub = parts.next().unwrap_or("").to_ascii_uppercase();
                if sub != "FETCH" {
                    writer
                        .write_all(format!("{tag} BAD unknown UID sub\r\n").as_bytes())
                        .await
                        .unwrap();
                    continue;
                }
                let range = parts.next().unwrap_or("1:*");
                if let Some(ranges) = &fetch_ranges {
                    ranges.lock().unwrap().push(range.to_string());
                }
                let start: u32 = range
                    .split(':')
                    .next()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(1);
                let mut matching: Vec<&(u32, Vec<u8>)> =
                    cfg.messages.iter().filter(|(u, _)| *u >= start).collect();
                if matching.is_empty() {
                    // `*` overlap: start > max → server returns the last.
                    if let Some(last) = cfg.messages.last() {
                        matching.push(last);
                    }
                }
                let mut seq = 0u32;
                for (uid, body) in &matching {
                    seq += 1;
                    let len = body.len();
                    writer
                        .write_all(
                            format!("* {seq} FETCH (UID {uid} BODY[] {{{len}}}\r\n").as_bytes(),
                        )
                        .await
                        .unwrap();
                    writer.write_all(body).await.unwrap();
                    writer.write_all(b")\r\n").await.unwrap();
                }
                writer
                    .write_all(format!("{tag} OK FETCH completed\r\n").as_bytes())
                    .await
                    .unwrap();
            }
            "IDLE" => {
                writer.write_all(b"+ idling\r\n").await.unwrap();
                if let Some(exists) = cfg.idle_push_exists {
                    // Deliver the newly-arrived mail, then push the EXISTS
                    // signal and await the client's DONE.
                    cfg.messages
                        .extend(std::mem::take(&mut cfg.idle_push_messages));
                    writer
                        .write_all(format!("* {exists} EXISTS\r\n").as_bytes())
                        .await
                        .unwrap();
                    // Read the DONE line (untagged).
                    let mut done = String::new();
                    reader.read_line(&mut done).await.unwrap();
                    assert!(done.trim().eq_ignore_ascii_case("DONE"));
                } else {
                    // No push: the client's `idle_wait` times out on its
                    // own; when it does `done()` it sends DONE.
                    let mut done = String::new();
                    reader.read_line(&mut done).await.unwrap();
                    assert!(done.trim().eq_ignore_ascii_case("DONE"));
                }
                writer
                    .write_all(format!("{tag} OK IDLE terminated\r\n").as_bytes())
                    .await
                    .unwrap();
            }
            "LOGOUT" => {
                writer
                    .write_all(format!("* BYE\r\n{tag} OK LOGOUT\r\n").as_bytes())
                    .await
                    .unwrap();
                break;
            }
            "" => break,
            other => {
                writer
                    .write_all(format!("{tag} OK {other} ok\r\n").as_bytes())
                    .await
                    .unwrap();
            }
        }
    }
}

/// Build a connector (app-password, poll mode) + a fake session wired to a
/// fake server with `cfg`. Returns the connector and the session.
async fn harness(cfg: FakeCfg) -> (EmailConnector, ImapSession<tokio::io::DuplexStream>) {
    let (client, server) = tokio::io::duplex(8 * 1024);
    let select_count = Arc::new(Mutex::new(0u32));
    tokio::spawn(run_fake(server, cfg, None, None, Arc::clone(&select_count)));
    let mut config = app_config();
    // Poll mode so run_sync skips IDLE and fetches immediately.
    config["mode"] = serde_json::json!("poll");
    let connector = EmailConnector::from_config(config, None, None).expect("config");
    let session = imap_login(
        Client::new(client),
        app_password_auth(),
        TEST_GREETING_BUDGET,
    )
    .await
    .expect("login");
    (connector, session)
}

/// Build a connector (app-password, idle mode) + a fake session wired to a
/// fake server with `cfg`, seeding the in-memory cursor from `cursor`. A
/// 1-second IDLE timeout keeps a missed-push test fast instead of hanging
/// on the default.
async fn idle_harness(
    cfg: FakeCfg,
    cursor: Option<&str>,
) -> (EmailConnector, ImapSession<tokio::io::DuplexStream>) {
    let (client, server) = tokio::io::duplex(8 * 1024);
    let select_count = Arc::new(Mutex::new(0u32));
    tokio::spawn(run_fake(server, cfg, None, None, Arc::clone(&select_count)));
    let mut config = app_config();
    config["mode"] = serde_json::json!("idle");
    config["idle_timeout_secs"] = 1.into();
    let connector =
        EmailConnector::from_config(config, None, cursor.map(str::to_string)).expect("config");
    let session = imap_login(
        Client::new(client),
        app_password_auth(),
        TEST_GREETING_BUDGET,
    )
    .await
    .expect("login");
    (connector, session)
}

fn app_password_auth() -> ImapAuth {
    ImapAuth::Login {
        username: "devansh@example.com".into(),
        password: "hunter2".into(),
    }
}

#[tokio::test]
async fn polling_incremental_sync_reports_cursor_without_advancing_in_memory() {
    let cfg = FakeCfg {
        messages: vec![
            (10u32, b"msg-10".to_vec()),
            (11, b"msg-11".to_vec()),
            (12, b"msg-12".to_vec()),
        ],
        ..Default::default()
    };
    let (connector, session) = harness(cfg).await;
    // First sync (full, no cursor): fetches all 3, reports cursor 17:12.
    let outcome = connector.run_sync(session, SyncOptions::default()).await;
    let outcome = outcome.expect("sync ok");
    assert_eq!(outcome.fetched, 3);
    assert_eq!(outcome.new_cursor.as_deref(), Some("17:12"));
    // Issue #332: the in-memory marker must NOT advance inside `sync` — the
    // supervisor persists the reported cursor and hands it back via
    // `on_cycle_succeeded` only after a fully successful cycle, so a failed
    // cycle re-syncs from the last confirmed cursor.
    assert_eq!(
        *connector.last_uid.lock().await,
        None,
        "in-memory cursor must not advance before the cycle succeeds"
    );
    let staged = connector.buffer.lock().await;
    assert_eq!(staged.len(), 3);
    assert_eq!(staged[0].uid, 10);
    assert_eq!(staged[2].uid, 12);
    assert_eq!(staged[2].raw, b"msg-12");
    drop(staged);

    // The supervisor's post-success adoption advances the marker.
    connector
        .on_cycle_succeeded(outcome.new_cursor.as_deref())
        .await;
    assert_eq!(*connector.last_uid.lock().await, Some((17, 12)));
}

#[tokio::test]
async fn polling_incremental_skips_already_synced() {
    let cfg = FakeCfg {
        messages: vec![
            (10u32, b"a".to_vec()),
            (11, b"b".to_vec()),
            (12, b"c".to_vec()),
        ],
        ..Default::default()
    };
    let (connector, session) = harness(cfg).await;
    // Seed the cursor at 11: only UID 12 should be fetched.
    *connector.last_uid.lock().await = Some((17, 11));
    let outcome = connector
        .run_sync(session, SyncOptions::default())
        .await
        .expect("sync ok");
    assert_eq!(outcome.fetched, 1);
    assert_eq!(outcome.new_cursor.as_deref(), Some("17:12"));
    let staged = connector.buffer.lock().await;
    assert_eq!(staged.len(), 1);
    assert_eq!(staged[0].uid, 12);
}

#[tokio::test]
async fn no_new_mail_leaves_cursor_unchanged() {
    let cfg = FakeCfg {
        messages: vec![(10u32, b"a".to_vec()), (11, b"b".to_vec())],
        ..Default::default()
    };
    let (connector, session) = harness(cfg).await;
    *connector.last_uid.lock().await = Some((17, 11));
    let outcome = connector
        .run_sync(session, SyncOptions::default())
        .await
        .expect("sync ok");
    assert_eq!(outcome.fetched, 0);
    assert!(outcome.new_cursor.is_none(), "no rewrite on a no-op cycle");
    assert_eq!(*connector.last_uid.lock().await, Some((17, 11)));
}

#[tokio::test]
async fn uidvalidity_reset_triggers_full_resync() {
    // The mailbox was recreated: the server now advertises a *new*
    // UIDVALIDITY (99) on SELECT, while the persisted cursor is from the
    // old epoch (17). The connector must detect the mismatch and do a
    // full re-fetch rather than trust the stale UID cursor.
    let cfg = FakeCfg {
        uid_validity: 99,
        messages: vec![(1u32, b"fresh-1".to_vec())],
        ..Default::default()
    };
    let (connector, session) = harness(cfg).await;
    *connector.last_uid.lock().await = Some((17, 11));
    let outcome = connector
        .run_sync(session, SyncOptions::default())
        .await
        .expect("sync ok");
    assert_eq!(outcome.fetched, 1);
    assert_eq!(outcome.new_cursor.as_deref(), Some("99:1"));
    // Issue #332: the marker stays at the last confirmed cursor until the
    // cycle succeeds.
    assert_eq!(*connector.last_uid.lock().await, Some((17, 11)));
    connector
        .on_cycle_succeeded(outcome.new_cursor.as_deref())
        .await;
    assert_eq!(*connector.last_uid.lock().await, Some((99, 1)));
}

/// Issue #332: a cycle that fails after `sync` (extract/insert/persist
/// error) must not lose the staged mail. The in-memory cursor only advances
/// via `on_cycle_succeeded`, so the next in-process cycle re-fetches the
/// failed window from the last confirmed cursor instead of skipping it.
#[tokio::test]
async fn failed_cycle_reprocesses_staged_mail_on_next_sync() {
    let messages = vec![
        (10u32, b"msg-10".to_vec()),
        (11, b"msg-11".to_vec()),
        (12, b"msg-12".to_vec()),
    ];
    let (connector, session) = harness(FakeCfg {
        messages: messages.clone(),
        ..Default::default()
    })
    .await;

    // Cycle 1: `sync` succeeds, but the cycle fails later (no adoption).
    let first = connector
        .run_sync(session, SyncOptions::default())
        .await
        .expect("sync ok");
    assert_eq!(first.fetched, 3);
    assert_eq!(first.new_cursor.as_deref(), Some("17:12"));
    assert_eq!(
        *connector.last_uid.lock().await,
        None,
        "a failed cycle must not advance the in-memory cursor"
    );

    // Cycle 2: re-syncs from the last confirmed cursor (none) and
    // re-processes the same window.
    let (_, session2) = harness(FakeCfg {
        messages: messages.clone(),
        ..Default::default()
    })
    .await;
    let second = connector
        .run_sync(session2, SyncOptions::default())
        .await
        .expect("sync ok");
    assert_eq!(second.fetched, 3, "the failed window must be re-fetched");
    assert_eq!(second.new_cursor.as_deref(), Some("17:12"));
    assert_eq!(
        connector.buffer.lock().await.len(),
        3,
        "the re-fetch must not duplicate the staged window"
    );

    // The cycle now succeeds: adopt the persisted cursor.
    connector
        .on_cycle_succeeded(second.new_cursor.as_deref())
        .await;
    assert_eq!(*connector.last_uid.lock().await, Some((17, 12)));

    // Cycle 3: incremental from the adopted cursor — no re-fetch.
    let (_, session3) = harness(FakeCfg {
        messages: messages.clone(),
        ..Default::default()
    })
    .await;
    let third = connector
        .run_sync(session3, SyncOptions::default())
        .await
        .expect("sync ok");
    assert_eq!(
        third.fetched, 0,
        "the adopted cursor makes the next cycle incremental"
    );
    assert!(third.new_cursor.is_none());
}

#[tokio::test]
async fn idle_push_triggers_incremental_fetch() {
    // IDLE mode: the connector blocks on IDLE until the server pushes an
    // EXISTS, then fetches the new message.
    let cfg = FakeCfg {
        messages: vec![(20u32, b"new-mail".to_vec())],
        idle_push_exists: Some(1),
        ..Default::default()
    };
    // Seed cursor at 17:19 so UID 20 is new.
    let (connector, session) = idle_harness(cfg, Some("17:19")).await;
    let outcome = connector
        .run_sync(session, SyncOptions::default())
        .await
        .expect("sync ok");
    assert_eq!(outcome.fetched, 1);
    assert_eq!(outcome.new_cursor.as_deref(), Some("17:20"));
    let staged = connector.buffer.lock().await;
    assert_eq!(staged.len(), 1);
    assert_eq!(staged[0].raw, b"new-mail");
}

#[tokio::test]
async fn idle_failed_cycle_resyncs_without_waiting_for_next_push() {
    // Issue #332: in IDLE mode, a cycle that fails after `sync` must not
    // block on IDLE for the next push — the IDLE notification for the failed
    // window will not re-fire, so the next cycle re-fetches from the last
    // confirmed cursor immediately.
    let cfg = FakeCfg {
        messages: vec![(20u32, b"new-mail".to_vec())],
        idle_push_exists: Some(1),
        ..Default::default()
    };
    let (connector, session) = idle_harness(cfg, Some("17:19")).await;
    let first = connector
        .run_sync(session, SyncOptions::default())
        .await
        .expect("sync ok");
    assert_eq!(first.fetched, 1);
    assert_eq!(first.new_cursor.as_deref(), Some("17:20"));
    assert_eq!(
        *connector.last_uid.lock().await,
        Some((17, 19)),
        "a failed cycle must not advance the in-memory cursor past the last confirmed value"
    );

    // Cycle 2: the server pushes nothing — the connector must skip the IDLE
    // wait and re-fetch the failed window immediately.
    let cfg2 = FakeCfg {
        messages: vec![(20u32, b"new-mail".to_vec())],
        idle_push_exists: None,
        ..Default::default()
    };
    let (_, session2) = idle_harness(cfg2, Some("17:19")).await;
    let second = connector
        .run_sync(session2, SyncOptions::default())
        .await
        .expect("sync ok");
    assert_eq!(
        second.fetched, 1,
        "the failed window must be re-fetched without an IDLE push"
    );
    assert_eq!(second.new_cursor.as_deref(), Some("17:20"));
    assert_eq!(
        connector.buffer.lock().await.len(),
        1,
        "the re-fetch must not duplicate staged mail"
    );
}

#[tokio::test]
async fn idle_timeout_no_new_mail_returns_zero() {
    // IDLE with no push: the connector's idle_wait uses a short timeout in
    // tests. Configure a 1-second idle timeout.
    let cfg = FakeCfg {
        idle_push_exists: None,
        messages: vec![(5u32, b"x".to_vec())],
        ..Default::default()
    };
    let (connector, session) = idle_harness(cfg, Some("17:5")).await;
    let outcome = connector
        .run_sync(session, SyncOptions::default())
        .await
        .expect("sync ok");
    assert_eq!(outcome.fetched, 0);
    assert!(outcome.new_cursor.is_none());
}

#[tokio::test]
async fn push_first_sync_backfills_existing_mail_before_idle() {
    // Issue #397: a fresh push-mode connector (no cursor) must import the
    // existing mailbox before blocking on IDLE — otherwise the first cycle
    // waits for the first new message while the current inbox is never
    // fetched. Here the server pushes nothing, so only a backfill-first
    // cycle can report the existing messages.
    let cfg = FakeCfg {
        messages: vec![
            (5u32, b"existing-1".to_vec()),
            (7u32, b"existing-2".to_vec()),
        ],
        idle_push_exists: None, // IDLE times out with no new mail
        ..Default::default()
    };
    let (connector, session) = idle_harness(cfg, None).await;
    let outcome = connector
        .run_sync(session, SyncOptions::default())
        .await
        .expect("sync ok");
    assert_eq!(outcome.fetched, 2, "existing mail must be backfilled");
    assert_eq!(outcome.new_cursor.as_deref(), Some("17:7"));
    let staged = connector.buffer.lock().await;
    assert_eq!(staged.len(), 2);
    assert_eq!(staged[0].raw, b"existing-1");
    assert_eq!(staged[1].raw, b"existing-2");
}

#[tokio::test]
async fn push_first_sync_backfills_then_fetches_only_new_mail() {
    // Issue #397: after the backfill, mail arriving during IDLE is fetched
    // incrementally from the backfilled UID — the whole mailbox must not be
    // re-fetched.
    let cfg = FakeCfg {
        messages: vec![(5u32, b"existing".to_vec())],
        idle_push_exists: Some(2),
        idle_push_messages: vec![(8u32, b"new-mail".to_vec())],
        ..Default::default()
    };
    let (client, server) = tokio::io::duplex(8 * 1024);
    let select_count = Arc::new(Mutex::new(0u32));
    let fetch_ranges: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    tokio::spawn(run_fake(
        server,
        cfg,
        None,
        Some(Arc::clone(&fetch_ranges)),
        Arc::clone(&select_count),
    ));
    let mut config = app_config();
    config["mode"] = serde_json::json!("idle");
    config["idle_timeout_secs"] = 1.into();
    let connector = EmailConnector::from_config(config, None, None).expect("config");
    let session = imap_login(
        Client::new(client),
        app_password_auth(),
        TEST_GREETING_BUDGET,
    )
    .await
    .expect("login");

    let outcome = connector
        .run_sync(session, SyncOptions::default())
        .await
        .expect("sync ok");
    assert_eq!(outcome.fetched, 2, "backfill + new mail");
    assert_eq!(outcome.new_cursor.as_deref(), Some("17:8"));
    let staged = connector.buffer.lock().await;
    assert_eq!(staged.len(), 2, "existing mail must not be staged twice");
    assert_eq!(staged[0].raw, b"existing");
    assert_eq!(staged[1].raw, b"new-mail");
    let ranges = fetch_ranges.lock().unwrap();
    assert_eq!(
        ranges.as_slice(),
        &["1:*".to_string(), "6:*".to_string()],
        "backfill fetches everything once, then only mail after the backfilled UID"
    );
}

#[tokio::test]
async fn push_backfill_persists_cursor_when_idle_push_carries_no_new_mail() {
    // Issue #397: after the backfill, an IDLE push that fires without a
    // fetchable message (e.g. a re-signalled EXISTS) must still persist the
    // backfill cursor — otherwise every following cycle sees no cursor and
    // re-fetches the whole mailbox until the first real new mail arrives.
    let cfg = FakeCfg {
        messages: vec![(5u32, b"existing".to_vec())],
        idle_push_exists: Some(1), // EXISTS re-signal; no appended messages
        idle_push_messages: Vec::new(),
        ..Default::default()
    };
    let (connector, session) = idle_harness(cfg, None).await;
    let outcome = connector
        .run_sync(session, SyncOptions::default())
        .await
        .expect("sync ok");
    assert_eq!(outcome.fetched, 1, "the backfill must be reported");
    assert_eq!(
        outcome.new_cursor.as_deref(),
        Some("17:5"),
        "the backfill cursor must persist even when the push carried no new mail"
    );
}

#[tokio::test]
async fn no_backfill_first_sync_seeds_cursor_instead_of_fetching() {
    // Issue #397: "only new content from now on" seeds the cursor to the
    // mailbox's current UIDNEXT so the first cycle never full-fetches
    // existing mail.
    let cfg = FakeCfg {
        messages: vec![(5u32, b"existing".to_vec())],
        ..Default::default()
    };
    let (connector, session) = no_backfill_harness(cfg).await;
    let outcome = connector
        .run_sync(session, SyncOptions::default())
        .await
        .expect("sync ok");
    assert_eq!(outcome.fetched, 0, "existing mail must not be fetched");
    assert_eq!(
        outcome.new_cursor.as_deref(),
        Some("17:5"),
        "cursor seeds to UIDNEXT - 1 (the last existing UID)"
    );
    assert!(connector.buffer.lock().await.is_empty());
}

#[tokio::test]
async fn no_backfill_uidvalidity_reset_still_full_resyncs() {
    // Issue #397 review: a persisted cursor whose UIDVALIDITY no longer
    // matches must full re-sync even with `initial_backfill: false` — the
    // mailbox was recreated, so seeding from UIDNEXT would skip every
    // message. The "only new content" seed applies to a true first sync
    // (no persisted cursor) only.
    let cfg = FakeCfg {
        uid_validity: 99,
        messages: vec![(1u32, b"fresh-1".to_vec())],
        ..Default::default()
    };
    let (connector, session) = no_backfill_harness(cfg).await;
    *connector.last_uid.lock().await = Some((17, 11));
    let outcome = connector
        .run_sync(session, SyncOptions::default())
        .await
        .expect("sync ok");
    assert_eq!(
        outcome.fetched, 1,
        "a recreated mailbox must be full re-fetched even with no-backfill config"
    );
    assert_eq!(
        outcome.new_cursor.as_deref(),
        Some("99:1"),
        "the re-sync must advance the cursor under the new UIDVALIDITY"
    );
}

#[tokio::test]
async fn no_backfill_first_sync_without_uidnext_errors() {
    // Issue #397 review: "only new content" anchors the seed on UIDNEXT; a
    // server that omits it must fail the sync rather than silently
    // full-fetching content the user opted out of.
    let cfg = FakeCfg {
        omit_uid_next: true,
        messages: vec![(1u32, b"existing".to_vec())],
        ..Default::default()
    };
    let (connector, session) = no_backfill_harness(cfg).await;
    let err = connector
        .run_sync(session, SyncOptions::default())
        .await
        .expect_err("a first sync without UIDNEXT must error");
    assert!(
        matches!(err, ConnectorError::Parse(_)),
        "expected Parse error, got {err:?}"
    );
    let msg = format!("{err}");
    assert!(msg.contains("UIDNEXT"), "error must name UIDNEXT: {msg}");
}

/// Build a connector (app-password, poll mode, `initial_backfill: false`)
/// + a fake session wired to a fake server with `cfg`.
///
/// Mirrors [`harness`] with the "only new content" first-sync config
/// (issue #397).
async fn no_backfill_harness(
    cfg: FakeCfg,
) -> (EmailConnector, ImapSession<tokio::io::DuplexStream>) {
    let (client, server) = tokio::io::duplex(8 * 1024);
    let select_count = Arc::new(Mutex::new(0u32));
    tokio::spawn(run_fake(server, cfg, None, None, Arc::clone(&select_count)));
    let mut config = app_config();
    config["mode"] = serde_json::json!("poll");
    config["initial_backfill"] = serde_json::json!(false);
    let connector = EmailConnector::from_config(config, None, None).expect("config");
    let session = imap_login(
        Client::new(client),
        app_password_auth(),
        TEST_GREETING_BUDGET,
    )
    .await
    .expect("login");
    (connector, session)
}

#[tokio::test]
async fn xoauth2_login_sends_correct_sasl_response() {
    // Verify the XOAUTH2 SASL initial response the connector would send to
    // Gmail/Microsoft: base64("user=..\x01auth=Bearer <token>\x01\x01").
    let cfg = FakeCfg::default();
    let (client, server) = tokio::io::duplex(8 * 1024);
    let captured: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
    let select_count = Arc::new(Mutex::new(0u32));
    tokio::spawn(run_fake(
        server,
        cfg,
        Some(Arc::clone(&captured)),
        None,
        Arc::clone(&select_count),
    ));
    let auth = ImapAuth::Xoauth2 {
        username: "devansh@example.com".into(),
        access_token: "ya29.token".into(),
    };
    let _session = imap_login(Client::new(client), auth, TEST_GREETING_BUDGET)
        .await
        .expect("xoauth2 login");
    let decoded = captured.lock().unwrap().clone();
    assert_eq!(
        decoded,
        b"user=devansh@example.com\x01auth=Bearer ya29.token\x01\x01".to_vec(),
        "XOAUTH2 SASL initial response must match the spec format"
    );
}

#[tokio::test]
async fn missing_uidvalidity_is_an_error_not_zero() {
    // RFC 3501 mandates the UIDVALIDITY response code. A server that omits
    // it must not collapse to epoch 0 (which collides with a persisted
    // `0:<uid>` cursor and would silently skip mail) — it must error.
    let cfg = FakeCfg {
        omit_uid_validity: true,
        messages: vec![(1u32, b"fresh-1".to_vec())],
        ..Default::default()
    };
    let (connector, session) = harness(cfg).await;
    let err = connector
        .run_sync(session, SyncOptions::default())
        .await
        .expect_err("missing UIDVALIDITY must error");
    assert!(
        matches!(err, ConnectorError::Parse(_)),
        "expected Parse error, got {err:?}"
    );
    let msg = format!("{err}");
    assert!(
        msg.contains("UIDVALIDITY"),
        "error must name UIDVALIDITY: {msg}"
    );
}

#[tokio::test]
async fn forced_idle_errors_when_server_lacks_idle() {
    // `Idle` mode documents "error if the server lacks the capability".
    // With the capability probe cached as Some(false), `use_idle()` must
    // return a Config error instead of silently polling.
    let mut config = app_config();
    config["mode"] = serde_json::json!("idle");
    let connector = EmailConnector::from_config(config, None, None).expect("config");
    *connector.supports_idle.lock().unwrap() = Some(false);
    let err = connector
        .use_idle()
        .expect_err("forced IDLE on an IDLE-less server must error");
    assert!(
        matches!(err, ConnectorError::Config(_)),
        "expected Config error, got {err:?}"
    );
    assert!(
        format!("{err}").contains("IDLE"),
        "error must mention IDLE: {err}"
    );
}

#[test]
fn auto_mode_defaults_to_push_when_unprobed_matching_mode() {
    // Before the first capability probe `supports_idle` is `None`.
    // `mode()` reports Push for Auto+None, so `use_idle()` must also opt
    // into IDLE (true) — a mismatch would let the supervisor's push loop
    // busy-spin on immediate polling returns.
    let mut config = app_config();
    // Default mode is `auto`; confirm explicitly.
    config["mode"] = serde_json::json!("auto");
    let connector = EmailConnector::from_config(config, None, None).expect("config");
    assert!(connector.use_idle().expect("auto+None should be Push"));
    assert!(matches!(connector.mode(), ConnectorMode::Push));
}

#[tokio::test]
async fn imap_sync_then_extract_yields_invite_facts() {
    // End-to-end over the fake IMAP transport: a real RFC 822 invite
    // fetched via `UID FETCH ... BODY.PEEK[]` is staged, then `extract()`
    // turns it into the appointment fact cluster. Proves the C5 transport
    // and C6 extraction compose without a live account.
    let invite = invite_email("REQUEST");
    let cfg = FakeCfg {
        messages: vec![(42u32, invite)],
        ..Default::default()
    };
    let (client, server) = tokio::io::duplex(8 * 1024);
    let select_count = Arc::new(Mutex::new(0u32));
    tokio::spawn(run_fake(server, cfg, None, None, Arc::clone(&select_count)));
    let mut config = app_config();
    config["mode"] = serde_json::json!("poll");
    let connector = EmailConnector::from_config_with_deps(
        config,
        EmailConnectorDeps {
            user_identity: Some("Devansh".into()),
            ..Default::default()
        },
    )
    .expect("config");
    let session = imap_login(
        Client::new(client),
        app_password_auth(),
        TEST_GREETING_BUDGET,
    )
    .await
    .expect("login");
    let outcome = connector
        .run_sync(session, SyncOptions::default())
        .await
        .expect("sync");
    assert_eq!(outcome.fetched, 1);

    let facts = connector.extract().await.expect("extract");
    assert!(
        facts.iter().any(|f| f.relationship_type == "has_event"),
        "invite extracted into a has_event fact: {facts:?}"
    );
    assert_eq!(
        facts
            .iter()
            .filter(|f| f.relationship_type == "attending")
            .count(),
        2
    );
    // Provenance `raw_reference` is the namespaced VEVENT UID (the stable
    // iMIP identity a CANCEL maps onto, issue #283), not the staged
    // message's IMAP UID.
    assert!(
        facts
            .iter()
            .all(|f| f.raw_reference.as_deref() == Some("imip:dentist-1@example.com")),
        "raw_reference must be the namespaced VEVENT UID: {facts:?}"
    );
    assert!(
        connector.buffer.lock().await.is_empty(),
        "extract drains the staged buffer"
    );
}
