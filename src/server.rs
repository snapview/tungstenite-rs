//! Methods to accept an incoming WebSocket connection on a server.

pub use handshake::server::ServerHandshake;

use handshake::HandshakeError;
use handshake::headers::Headers;
use protocol::WebSocket;

use std::io::{Read, Write};

/// Accept the given Stream as a WebSocket.
///
/// This function starts a server WebSocket handshake over the given stream.
/// If you want TLS support, use `native_tls::TlsStream` or `openssl::ssl::SslStream`
/// for the stream here. Any `Read + Write` streams are supported, including
/// those from `Mio` and others.
pub fn accept<Stream: Read + Write>(stream: Stream)
    -> Result<(WebSocket<Stream>, Headers), HandshakeError<Stream, ServerHandshake>>
{
    ServerHandshake::start(stream).handshake()
}
