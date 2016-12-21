use std::net::{TcpStream, ToSocketAddrs};
use url::{Url, SocketAddrs};

use protocol::WebSocket;
use handshake::{Handshake as HandshakeTrait, HandshakeResult};
use handshake::client::{ClientHandshake, Request};
use error::{Error, Result};

/// Connect to the given WebSocket.
///
/// Note that this function may block the current thread while DNS resolution is performed.
pub fn connect(url: Url) -> Result<Handshake> {
    let mode = match url.scheme() {
        "ws" => Mode::Plain,
        #[cfg(feature="tls")]
        "wss" => Mode::Tls,
        _ => return Err(Error::Url("URL scheme not supported".into()))
    };

    // Note that this function may block the current thread while resolution is performed.
    let addrs = url.to_socket_addrs()?;
    Ok(Handshake {
        state: HandshakeState::Nothing(url),
        alt_addresses: addrs,
    })
}

enum Mode {
    Plain,
    Tls,
}

enum HandshakeState {
    Nothing(Url),
    WebSocket(ClientHandshake<TcpStream>),
}

pub struct Handshake {
    state: HandshakeState,
    alt_addresses: SocketAddrs,
}

impl HandshakeTrait for Handshake {
    type Stream = WebSocket<TcpStream>;
    fn handshake(mut self) -> Result<HandshakeResult<Self>> {
        match self.state {
            HandshakeState::Nothing(url) => {
                if let Some(addr) = self.alt_addresses.next() {
                    debug!("Trying to contact {} at {}...", url, addr);
                    let state = {
                        if let Ok(stream) = TcpStream::connect(addr) {
                            let hs = ClientHandshake::new(stream, Request { url: url });
                            HandshakeState::WebSocket(hs)
                        } else {
                            HandshakeState::Nothing(url)
                        }
                    };
                    Ok(HandshakeResult::Incomplete(Handshake {
                        state: state,
                        ..self
                    }))
                } else {
                    Err(Error::Url(format!("Unable to resolve {}", url).into()))
                }
            }
            HandshakeState::WebSocket(ws) => {
                let alt_addresses = self.alt_addresses;
                ws.handshake().map(move |r| r.map(move |s| Handshake {
                    state: HandshakeState::WebSocket(s),
                    alt_addresses: alt_addresses,
                }))
            }
        }
    }
}
