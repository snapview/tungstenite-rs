//! Methods to accept an incoming WebSocket connection on a server.

pub use handshake::server::ServerHandshake;

use handshake::HandshakeError;
use handshake::server::{Callback, NoCallback};

use protocol::WebSocket;

use std::io::{Read, Write};

/// Accept the given Stream as a WebSocket.
///
/// This function starts a server WebSocket handshake over the given stream.
/// If you want TLS support, use `native_tls::TlsStream` or `openssl::ssl::SslStream`
/// for the stream here. Any `Read + Write` streams are supported, including
/// those from `Mio` and others.
pub fn accept<S: Read + Write>(
    stream: S,
) -> Result<WebSocket<S>, HandshakeError<ServerHandshake<S, NoCallback>>> {
    accept_hdr(stream, NoCallback)
}

/// Accept the given Stream as a WebSocket.
///
/// This function does the same as `accept()` but accepts an extra callback
/// for header processing. The callback receives headers of the incoming
/// requests and is able to add extra headers to the reply.
pub fn accept_hdr<S: Read + Write, C: Callback>(
    stream: S,
    callback: C,
) -> Result<WebSocket<S>, HandshakeError<ServerHandshake<S, C>>> {
    ServerHandshake::start(stream, callback).handshake()
}
