//! Methods to accept an incoming WebSocket connection on a server.

pub use crate::handshake::server::ServerHandshake;

use crate::handshake::server::{Callback, NoCallback};
use crate::handshake::HandshakeError;

use crate::protocol::{WebSocket, WebSocketConfig};

use crate::ext::uncompressed::UncompressedExt;
use crate::ext::WebSocketExtension;
use std::io::{Read, Write};

/// Accept the given Stream as a WebSocket.
///
/// Uses a configuration provided as an argument. Calling it with `None` will use the default one
/// used by `accept()`.
///
/// This function starts a server WebSocket handshake over the given stream.
/// If you want TLS support, use `native_tls::TlsStream` or `openssl::ssl::SslStream`
/// for the stream here. Any `Read + Write` streams are supported, including
/// those from `Mio` and others.
pub fn accept_with_config<S: Read + Write, E>(
    stream: S,
    config: Option<WebSocketConfig<E>>,
) -> Result<WebSocket<S, E>, HandshakeError<ServerHandshake<S, NoCallback, E>>>
where
    E: WebSocketExtension,
{
    accept_hdr_with_config(stream, NoCallback, config)
}

/// Accept the given Stream as a WebSocket.
///
/// This function starts a server WebSocket handshake over the given stream.
/// If you want TLS support, use `native_tls::TlsStream` or `openssl::ssl::SslStream`
/// for the stream here. Any `Read + Write` streams are supported, including
/// those from `Mio` and others.
pub fn accept<S: Read + Write>(
    stream: S,
) -> Result<
    WebSocket<S, UncompressedExt>,
    HandshakeError<ServerHandshake<S, NoCallback, UncompressedExt>>,
> {
    accept_with_config(stream, None)
}

/// Accept the given Stream as a WebSocket.
///
/// Uses a configuration provided as an argument. Calling it with `None` will use the default one
/// used by `accept_hdr()`.
///
/// This function does the same as `accept()` but accepts an extra callback
/// for header processing. The callback receives headers of the incoming
/// requests and is able to add extra headers to the reply.
pub fn accept_hdr_with_config<S: Read + Write, C: Callback, E>(
    stream: S,
    callback: C,
    config: Option<WebSocketConfig<E>>,
) -> Result<WebSocket<S, E>, HandshakeError<ServerHandshake<S, C, E>>>
where
    E: WebSocketExtension,
{
    ServerHandshake::start(stream, callback, config).handshake()
}

/// Accept the given Stream as a WebSocket.
///
/// This function does the same as `accept()` but accepts an extra callback
/// for header processing. The callback receives headers of the incoming
/// requests and is able to add extra headers to the reply.
pub fn accept_hdr<S: Read + Write, C: Callback>(
    stream: S,
    callback: C,
) -> Result<WebSocket<S, UncompressedExt>, HandshakeError<ServerHandshake<S, C, UncompressedExt>>> {
    accept_hdr_with_config(stream, callback, None)
}
