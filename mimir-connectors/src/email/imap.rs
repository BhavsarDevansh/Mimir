//! IMAP transport for the Email connector (Phase 3 C5 / #199).
//!
//! A thin, dependency-light wrapper over [`async_imap`] that speaks only the
//! IMAP verbs this connector needs: `LOGIN` (app password) / `AUTHENTICATE
//! XOAUTH2` (OAuth), `SELECT` (for `UIDVALIDITY`), `UID FETCH` (incremental
//! sync by UID), and `IDLE` (RFC 2177 push). Mail parsing / fact extraction is
//! C6 / #200 — this transport stages raw RFC 822 messages only.
//!
//! # TLS
//!
//! IMAP runs over implicit TLS (`imaps:993`). Rather than use async-imap's
//! `connect()` helper (which pulls `async-native-tls` / a system OpenSSL), we
//! hand-roll the TCP + [`tokio_rustls`] handshake so the workspace keeps a
//! single rustls TLS stack. The crypto provider is `aws-lc-rs`, the same one
//! reqwest's `rustls` feature already compiles — no second provider enters
//! the tree. The resulting `TlsStream` is handed to
//! [`async_imap::Client::new`], which accepts any `tokio::io::AsyncRead +
//! AsyncWrite` stream (not only async-native-tls).
//!
//! # Testability
//!
//! The session logic is generic over the underlying stream `S` and exposed as
//! [`ImapSession`], so tests drive it against a fake IMAP server speaking the
//! protocol over a [`tokio::io::duplex`] pair — no TLS, no live account. The
//! production path ([`connect_tls`] + [`imap_login`]) is the only TLS-aware
//! entry point.
//!
//! # No `unsafe`
//!
//! This module honours the workspace `#![deny(unsafe_code)]` guarantee.

use std::sync::Arc;
use std::time::Duration;

use async_imap::Authenticator;
use async_imap::Session;
use async_imap::extensions::idle::IdleResponse;
use chrono::{DateTime, FixedOffset};
use futures::StreamExt;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::rustls::RootCertStore;
use tokio_rustls::rustls::pki_types::ServerName;
use tracing::debug;

use crate::connector::ConnectorError;

/// Underlying stream trait alias: any tokio async read/write pair that is
/// `Unpin`, `Debug`, and `Send`, matching async-imap's `Client<S>` bound.
pub(crate) trait ImapStream:
    AsyncRead + AsyncWrite + Unpin + std::fmt::Debug + Send
{
}
impl<T> ImapStream for T where T: AsyncRead + AsyncWrite + Unpin + std::fmt::Debug + Send {}

// ---------------------------------------------------------------------------
// Auth
// ---------------------------------------------------------------------------

/// Resolved, ready-to-use IMAP credentials — the non-secret config plus the
/// live secret, produced by the connector after loading/refreshing a
/// [`crate::secrets::SecretBundle`].
#[derive(Clone)]
pub(crate) enum ImapAuth {
    /// Plain `LOGIN` with a username + app-specific password.
    Login { username: String, password: String },
    /// `AUTHENTICATE XOAUTH2` (RFC 6749 + Gmail's XOAUTH2 mechanism). The
    /// access token is the OAuth bearer; `username` is the account email
    /// embedded in the SASL initial response.
    Xoauth2 {
        username: String,
        access_token: String,
    },
}

// `ImapAuth` carries the live password / OAuth access token in cleartext, so
// its `Debug` impl redacts the secret values — matching the `SecretBundle`
// redaction standard — so a stray `tracing::debug!(?auth)` or error formatter
// never leaks credentials to logs or the persisted `last_error` string.
impl std::fmt::Debug for ImapAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Login { username, .. } => f
                .debug_struct("Login")
                .field("username", username)
                .field("password", &"<redacted>")
                .finish(),
            Self::Xoauth2 { username, .. } => f
                .debug_struct("Xoauth2")
                .field("username", username)
                .field("access_token", &"<redacted>")
                .finish(),
        }
    }
}

/// [`async_imap::Authenticator`] for the XOAUTH2 SASL mechanism.
///
/// The initial client response (sent on the empty first continuation) is
/// `base64("user=<u>\x01auth=Bearer <token>\x01\x01")`; async-imap base64-
/// encodes whatever `process` returns, so we return the *raw* bytes. If the
/// server later sends an error challenge, the client cancels by replying with
/// an empty string (Gmail then returns a tagged BAD/NO).
struct Xoauth2Authenticator {
    username: String,
    access_token: String,
    /// `true` once the initial response has been sent, so a later (error)
    /// challenge is answered with an empty cancellation string rather than
    /// re-sending the credentials.
    sent_initial: bool,
}

impl Authenticator for Xoauth2Authenticator {
    type Response = Vec<u8>;

    fn process(&mut self, challenge: &[u8]) -> Vec<u8> {
        if !self.sent_initial && challenge.is_empty() {
            self.sent_initial = true;
            // Control chars are ^A (0x01) per the XOAUTH2 spec.
            format!(
                "user={}\x01auth=Bearer {}\x01\x01",
                self.username, self.access_token
            )
            .into_bytes()
        } else {
            // Either a re-challenge after the initial response (server error
            // payload) or a non-empty initial challenge (unsupported): cancel.
            Vec::new()
        }
    }
}

/// Authenticate an unauthenticated [`async_imap::Client`] into an
/// [`ImapSession`], choosing `LOGIN` or `AUTHENTICATE XOAUTH2` per the
/// resolved [`ImapAuth`] kind. On failure the underlying client is returned
/// by async-imap alongside the error; we surface the error (the client is not
/// reusable after a broken handshake, so dropping it is correct).
pub(crate) async fn imap_login<S: ImapStream>(
    mut client: async_imap::Client<S>,
    auth: ImapAuth,
) -> Result<ImapSession<S>, ConnectorError> {
    // Drain the IMAP greeting (untagged `* OK`) before issuing LOGIN /
    // AUTHENTICATE. async-imap's `connect()` helper does this for us; with
    // `Client::new` (which we use to keep a single rustls TLS stack) the
    // greeting would otherwise sit in the socket buffer and deadlock
    // `authenticate`'s challenge/response handshake: `do_auth_handshake`
    // routes an untagged greeting into `check_done_ok_from`, which then
    // consumes the server's continuation instead of replying to it. `login`
    // tolerates the stray greeting, but we drain it for both paths for
    // parity and determinism.
    client
        .read_response()
        .await
        .map_err(|e| ConnectorError::Network(format!("IMAP greeting read failed: {e}")))?
        .ok_or_else(|| ConnectorError::Network("IMAP server closed before greeting".into()))?;

    let session = match auth {
        ImapAuth::Login { username, password } => client
            .login(&username, &password)
            .await
            .map_err(|(err, _client)| map_login_error(err))?,
        ImapAuth::Xoauth2 {
            username,
            access_token,
        } => {
            let authenticator = Xoauth2Authenticator {
                username,
                access_token,
                sent_initial: false,
            };
            client
                .authenticate("XOAUTH2", authenticator)
                .await
                .map_err(|(err, _client)| map_login_error(err))?
        }
    };
    Ok(ImapSession::new(session))
}

fn map_login_error(err: async_imap::error::Error) -> ConnectorError {
    use async_imap::error::Error as E;
    match err {
        // A BAD/NO on LOGIN/AUTHENTICATE is a credential rejection.
        E::Bad(msg) => ConnectorError::Authentication(format!("IMAP auth rejected (BAD): {msg}")),
        E::No(msg) => ConnectorError::Authentication(format!("IMAP auth rejected (NO): {msg}")),
        E::ConnectionLost => ConnectorError::Network("IMAP connection lost during login".into()),
        E::Io(e) => ConnectorError::Network(format!("IMAP login I/O: {e}")),
        other => ConnectorError::Other(format!("IMAP login failed: {other}")),
    }
}

// ---------------------------------------------------------------------------
// Raw staged message
// ---------------------------------------------------------------------------

/// A raw, un-parsed email staged from an IMAP `UID FETCH`, awaiting C6 / #200
/// fact extraction. Stores exactly what C6 needs: the UID (the connector
/// cursor / provenance id), the server `INTERNALDATE` (received-time), and the
/// full RFC 822 message bytes (`BODY.PEEK[]`, headers + body).
///
/// The fields are written by C5 and read by C6; until C6 lands they are only
/// read in tests, so the staged-payload dead-code lint is silenced at the
/// struct level.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(crate) struct RawEmail {
    /// The message's unique id within the current `UIDVALIDITY` epoch.
    pub uid: u32,
    /// `UIDVALIDITY` of the mailbox the message was fetched from. An IMAP UID
    /// is unique only within one mailbox and one `UIDVALIDITY` epoch, so the
    /// provenance `raw_reference` is built as `{uid_validity}:{uid}` (matching
    /// the persisted cursor format) to stay globally unique across epochs.
    pub uid_validity: u32,
    /// Server-side receive timestamp (`INTERNALDATE`).
    pub internal_date: Option<DateTime<FixedOffset>>,
    /// Full RFC 822 message bytes (`BODY.PEEK[]`): headers + body.
    pub raw: Vec<u8>,
}

/// Result of an incremental `UID FETCH`.
#[derive(Debug)]
pub(crate) struct FetchResult {
    /// Staged messages, UIDs strictly greater than the request floor.
    pub messages: Vec<RawEmail>,
    /// The highest UID observed (the new cursor floor), `0` when nothing was
    /// fetched.
    pub max_uid: u32,
}

/// [`SELECT`] response metadata the connector needs for cursor validity.
#[derive(Debug, Clone, Copy)]
pub(crate) struct MailboxInfo {
    /// `UIDVALIDITY` of the selected mailbox; changes when the mailbox is
    /// recreated, invalidating all prior UIDs.
    pub uid_validity: u32,
}

// ---------------------------------------------------------------------------
// Session wrapper
// ---------------------------------------------------------------------------

/// A thin wrapper over an authenticated [`async_imap::Session`] generic in the
/// underlying stream, so the same sync logic runs against a live TLS socket or
/// a test duplex pair.
pub(crate) struct ImapSession<S: ImapStream> {
    session: Session<S>,
}

impl<S: ImapStream> ImapSession<S> {
    /// Wrap an authenticated session.
    pub(crate) fn new(session: Session<S>) -> Self {
        Self { session }
    }

    /// `EXAMINE <mailbox>` (read-only; the connector never mutates mailbox
    /// state — `BODY.PEEK[]` already avoids marking messages `\Seen`).
    /// Returns the `UIDVALIDITY` the connector uses to validate its persisted
    /// cursor.
    pub(crate) async fn examine(&mut self, mailbox: &str) -> Result<MailboxInfo, ConnectorError> {
        let mbox = self
            .session
            .examine(mailbox)
            .await
            .map_err(map_imap_error)?;
        let uid_validity = mbox.uid_validity.ok_or_else(|| {
            ConnectorError::Parse(format!(
                "IMAP EXAMINE for `{mailbox}` returned no UIDVALIDITY"
            ))
        })?;
        Ok(MailboxInfo { uid_validity })
    }

    /// Probe the server `CAPABILITY` response; `true` if `IDLE` is advertised.
    ///
    /// Used by [`crate::email::EmailConnector::authenticate`] to decide Push
    /// vs Polling mode (auto-detect) and cached for [`Connector::mode`].
    pub(crate) async fn supports_idle(&mut self) -> Result<bool, ConnectorError> {
        let caps = self.session.capabilities().await.map_err(map_imap_error)?;
        Ok(caps.has(&async_imap::types::Capability::Atom("IDLE".into())))
    }

    /// Incremental `UID FETCH`: every message with a UID strictly greater than
    /// `since` (or all messages when `since` is `None`). Uses `BODY.PEEK[]` so
    /// messages are not marked `\Seen`. The `*` range-end is RFC 3501's "max
    /// UID" and may re-return the last message when `since+1` exceeds it, so
    /// returned UIDs `<= since` are filtered out (no re-fetch, per #199).
    pub(crate) async fn fetch_since(
        &mut self,
        since: Option<u32>,
        uid_validity: u32,
    ) -> Result<FetchResult, ConnectorError> {
        let range = match since {
            Some(uid) => format!("{}:*", uid.saturating_add(1)),
            None => "1:*".to_string(),
        };
        let query = "(UID INTERNALDATE BODY.PEEK[])";
        let mut stream = self
            .session
            .uid_fetch(&range, query)
            .await
            .map_err(map_imap_error)?;
        let floor = since.unwrap_or(0);
        let mut messages = Vec::new();
        let mut max_uid = floor;
        while let Some(fetch) = stream.next().await {
            let fetch = fetch.map_err(map_imap_error)?;
            let Some(uid) = fetch.uid else {
                // A FETCH without a UID (should not happen for UID FETCH) — skip.
                debug!("IMAP FETCH row carried no UID; skipping");
                continue;
            };
            if uid <= floor {
                // `*` overlap: the server re-returned the last-known message.
                continue;
            }
            if uid > max_uid {
                max_uid = uid;
            }
            let raw = fetch.body().map(<[u8]>::to_vec).unwrap_or_default();
            messages.push(RawEmail {
                uid,
                uid_validity,
                internal_date: fetch.internal_date(),
                raw,
            });
        }
        Ok(FetchResult { messages, max_uid })
    }

    /// `IDLE` until the server signals new mail or `timeout` elapses (RFC
    /// 2177). Consumes `self` and returns the session plus a flag: `true` if
    /// new data arrived (run an incremental fetch), `false` on timeout / manual
    /// interrupt (no fetch this cycle). The connector re-issues IDLE every
    /// cycle, so a `timeout` shorter than the server's ~29 min inactivity
    /// limit is correct and recommended.
    pub(crate) async fn idle_wait(self, timeout: Duration) -> Result<(Self, bool), ConnectorError> {
        let mut handle = self.session.idle();
        handle.init().await.map_err(map_imap_error)?;
        let (fut, _stop) = handle.wait_with_timeout(timeout);
        let new_data = match fut.await {
            Ok(IdleResponse::NewData(_)) => true,
            Ok(IdleResponse::Timeout) | Ok(IdleResponse::ManualInterrupt) => false,
            Err(err) => return Err(map_imap_error(err)),
        };
        let session = handle.done().await.map_err(map_imap_error)?;
        Ok((ImapSession::new(session), new_data))
    }

    /// Log out gracefully (best-effort; errors are ignored as the connection
    /// is closing anyway).
    pub(crate) async fn logout(mut self) {
        if let Err(err) = self.session.logout().await {
            debug!(error = %err, "IMAP logout failed (ignored)");
        }
    }
}

/// Map an [`async_imap`] error onto a [`ConnectorError`] variant.
fn map_imap_error(err: async_imap::error::Error) -> ConnectorError {
    use async_imap::error::Error as E;
    match err {
        E::ConnectionLost => ConnectorError::Network("IMAP connection lost".into()),
        E::Io(e) => ConnectorError::Network(format!("IMAP I/O: {e}")),
        E::Bad(msg) => ConnectorError::Parse(format!("IMAP BAD: {msg}")),
        // A post-login NO (e.g. a vanished mailbox) is a generic command
        // refusal, not a credential failure — login auth is mapped separately
        // in `map_login_error`.
        E::No(msg) => ConnectorError::Other(format!("IMAP NO: {msg}")),
        other => ConnectorError::Other(format!("IMAP error: {other}")),
    }
}

// ---------------------------------------------------------------------------
// Production TLS connect
// ---------------------------------------------------------------------------

/// Open an implicit-TLS (`imaps:993`) connection to `host:port` using a rustls
/// client configured with the system trust store (matching reqwest's
/// `rustls-native-certs` feature) and the `aws-lc-rs` crypto provider reqwest
/// already compiles. Returns the established `TlsStream` ready for
/// [`async_imap::Client::new`].
pub(crate) async fn connect_tls(host: &str, port: u16) -> Result<TlsStream, ConnectorError> {
    let mut roots = RootCertStore::empty();
    let native = rustls_native_certs::load_native_certs();
    for cert in native.certs {
        let _ = roots.add(cert);
    }
    if roots.is_empty() {
        return Err(ConnectorError::Config(
            "no native CA certificates found for IMAP TLS".into(),
        ));
    }
    if !native.errors.is_empty() {
        debug!(
            errors = native.errors.len(),
            loaded = roots.len(),
            "some native CA sources failed during IMAP TLS trust-store load"
        );
    }
    let provider = Arc::new(tokio_rustls::rustls::crypto::aws_lc_rs::default_provider());
    let config = tokio_rustls::rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| ConnectorError::Config(format!("rustls protocol versions: {e}")))?
        .with_root_certificates(roots)
        .with_no_client_auth();
    let connector = TlsConnector::from(Arc::new(config));
    let tcp = TcpStream::connect((host, port))
        .await
        .map_err(|e| ConnectorError::Network(format!("connect {host}:{port}: {e}")))?;
    let server_name = ServerName::try_from(host.to_string())
        .map_err(|e| ConnectorError::Config(format!("invalid IMAP host `{host}`: {e}")))?;
    let tls = connector
        .connect(server_name, tcp)
        .await
        .map_err(|e| ConnectorError::Network(format!("TLS handshake {host}:{port}: {e}")))?;
    Ok(tls)
}

/// Owned type alias for the production TLS stream: a tokio-rustls client
/// stream over a tokio TCP socket. Implements `tokio::io::AsyncRead` +
/// `AsyncWrite` + `Debug` + `Send`, so it satisfies [`ImapStream`] and feeds
/// directly into [`async_imap::Client::new`].
pub(crate) type TlsStream = tokio_rustls::client::TlsStream<TcpStream>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xoauth2_initial_response_format() {
        let mut a = Xoauth2Authenticator {
            username: "devansh@example.com".into(),
            access_token: "ya29.token".into(),
            sent_initial: false,
        };
        let resp = a.process(&[]);
        assert_eq!(
            resp,
            b"user=devansh@example.com\x01auth=Bearer ya29.token\x01\x01".to_vec()
        );
        assert!(a.sent_initial);
        // A later (error) challenge cancels with an empty response.
        assert_eq!(a.process(b"some error"), Vec::<u8>::new());
    }

    #[test]
    fn xoauth2_nonempty_initial_challenge_cancels() {
        let mut a = Xoauth2Authenticator {
            username: "u".into(),
            access_token: "t".into(),
            sent_initial: false,
        };
        assert_eq!(a.process(b"challenge"), Vec::<u8>::new());
        assert!(!a.sent_initial);
    }

    #[test]
    fn imap_auth_debug_redacts_secrets() {
        let cases = [
            ImapAuth::Login {
                username: "devansh@example.com".into(),
                password: "super-secret-password".into(),
            },
            ImapAuth::Xoauth2 {
                username: "devansh@example.com".into(),
                access_token: "ya29.super-secret-token".into(),
            },
        ];
        for auth in &cases {
            let dbg = format!("{auth:?}");
            assert!(
                !dbg.contains("super-secret"),
                "ImapAuth Debug leaked a secret: {dbg}"
            );
            // Non-secret context (the discriminant + username) is preserved.
            assert!(
                dbg.contains("devansh@example.com"),
                "ImapAuth Debug lost the username: {dbg}"
            );
            assert!(dbg.contains("<redacted>"), "ImapAuth must redact: {dbg}");
        }
    }
}
