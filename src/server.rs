pub use handshake::server::ServerHandshake;

use handshake::HandshakeError;
use protocol::WebSocket;

use std::io::{Read, Write};

/// Accept the given TcpStream as a WebSocket.
pub fn accept<Stream: Read + Write>(stream: Stream)
    -> Result<WebSocket<Stream>, HandshakeError<Stream, ServerHandshake>>
{
    ServerHandshake::start(stream).handshake()
}
