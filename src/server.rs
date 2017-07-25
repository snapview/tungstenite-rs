//! Methods to accept an incoming WebSocket connection on a server.

pub use handshake::server::ServerHandshake;

use handshake::HandshakeError;
use handshake::server::Callback;
use protocol::WebSocket;

use std::io::{Read, Write};

/// Accept the given Stream as a WebSocket.
///
/// This function starts a server WebSocket handshake over the given stream.
/// If you want TLS support, use `native_tls::TlsStream` or `openssl::ssl::SslStream`
/// for the stream here. Any `Read + Write` streams are supported, including
/// those from `Mio` and others. You can also pass an optional `callback` which will
/// be called when the websocket request is received from an incoming client.
pub fn accept<S: Read + Write>(stream: S, callback: Option<Callback>)
    -> Result<WebSocket<S>, HandshakeError<ServerHandshake<S>>>
{
    ServerHandshake::start(stream, callback).handshake()
}
