use std::net::TcpStream;

use handshake::server::ServerHandshake;

/// Accept the given TcpStream as a WebSocket.
pub fn accept(stream: TcpStream) -> ServerHandshake<TcpStream> {
    ServerHandshake::new(stream)
}
