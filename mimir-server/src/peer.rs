//! Transport-independent peer identification for the daemon.
#![deny(unsafe_code)]

use std::fmt;
use std::net::SocketAddr;

use axum::extract::connect_info::Connected;
use axum::serve::IncomingStream;
use tokio::net::TcpListener;
#[cfg(unix)]
use tokio::net::UnixListener;

/// The peer that initiated a request, regardless of the transport it arrived
/// on (TCP loopback or a Unix domain socket, issue #25).
///
/// Both daemon listeners are served with this connect-info type so the
/// loopback guard and the `/stop` handler work identically on either
/// transport: a Unix peer is always local (filesystem permissions on the
/// socket file are the access control), while a TCP peer must still be a
/// loopback address.
#[derive(Clone, Debug)]
pub enum LocalPeer {
    /// TCP peer (`127.0.0.1:8080` etc.).
    Tcp(SocketAddr),
    /// Unix-domain-socket peer (`~/.local/share/mimir/mimir.sock` etc.).
    #[cfg(unix)]
    Unix(tokio::net::unix::SocketAddr),
}

impl LocalPeer {
    /// Whether the peer is local to this machine.
    ///
    /// TCP peers must be loopback addresses; Unix peers are always local
    /// because connecting to a Unix socket already required filesystem access
    /// to the socket file.
    pub fn is_loopback(&self) -> bool {
        match self {
            Self::Tcp(addr) => addr.ip().is_loopback(),
            #[cfg(unix)]
            Self::Unix(_) => true,
        }
    }
}

impl fmt::Display for LocalPeer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Tcp(addr) => write!(f, "{addr}"),
            #[cfg(unix)]
            Self::Unix(addr) => write!(f, "unix:{addr:?}"),
        }
    }
}

impl Connected<IncomingStream<'_, TcpListener>> for LocalPeer {
    fn connect_info(stream: IncomingStream<'_, TcpListener>) -> Self {
        Self::Tcp(*stream.remote_addr())
    }
}

#[cfg(unix)]
impl Connected<IncomingStream<'_, UnixListener>> for LocalPeer {
    fn connect_info(stream: IncomingStream<'_, UnixListener>) -> Self {
        Self::Unix(stream.remote_addr().clone())
    }
}
