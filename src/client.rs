//! Methods to connect to an WebSocket as a client.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::result::Result as StdResult;

use log::*;
use url::Url;

use crate::handshake::client::Response;
use crate::protocol::WebSocketConfig;

#[cfg(feature = "tls")]
mod encryption {
    pub use native_tls::TlsStream;
    use native_tls::{HandshakeError as TlsHandshakeError, TlsConnector};
    use std::net::TcpStream;

    pub use crate::stream::Stream as StreamSwitcher;
    /// TCP stream switcher (plain/TLS).
    pub type AutoStream = StreamSwitcher<TcpStream, TlsStream<TcpStream>>;

    use crate::error::Result;
    use crate::stream::Mode;

    pub fn wrap_stream(stream: TcpStream, domain: &str, mode: Mode) -> Result<AutoStream> {
        match mode {
            Mode::Plain => Ok(StreamSwitcher::Plain(stream)),
            Mode::Tls => {
                let connector = TlsConnector::builder().build()?;
                connector
                    .connect(domain, stream)
                    .map_err(|e| match e {
                        TlsHandshakeError::Failure(f) => f.into(),
                        TlsHandshakeError::WouldBlock(_) => {
                            panic!("Bug: TLS handshake not blocked")
                        }
                    })
                    .map(StreamSwitcher::Tls)
            }
        }
    }
}

#[cfg(not(feature = "tls"))]
mod encryption {
    use std::net::TcpStream;

    use crate::error::{Error, Result};
    use crate::stream::Mode;

    /// TLS support is nod compiled in, this is just standard `TcpStream`.
    pub type AutoStream = TcpStream;

    pub fn wrap_stream(stream: TcpStream, _domain: &str, mode: Mode) -> Result<AutoStream> {
        match mode {
            Mode::Plain => Ok(stream),
            Mode::Tls => Err(Error::Url("TLS support not compiled in.".into())),
        }
    }
}

use self::encryption::wrap_stream;
pub use self::encryption::AutoStream;

use crate::error::{Error, Result};
use crate::handshake::client::{ClientHandshake, Request};
use crate::handshake::HandshakeError;
use crate::protocol::WebSocket;
use crate::stream::{Mode, NoDelay};

/// Connect to the given WebSocket in blocking mode.
///
/// Uses a websocket configuration passed as an argument to the function. Calling it with `None` is
/// equal to calling `connect()` function.
///
/// The URL may be either ws:// or wss://.
/// To support wss:// URLs, feature "tls" must be turned on.
///
/// This function "just works" for those who wants a simple blocking solution
/// similar to `std::net::TcpStream`. If you want a non-blocking or other
/// custom stream, call `client` instead.
///
/// This function uses `native_tls` to do TLS. If you want to use other TLS libraries,
/// use `client` instead. There is no need to enable the "tls" feature if you don't call
/// `connect` since it's the only function that uses native_tls.
pub fn connect_with_config<'t, Req: Into<Request<'t>>>(
    request: Req,
    config: Option<WebSocketConfig>,
) -> Result<(WebSocket<AutoStream>, Response)> {
    let request: Request = request.into();
    let mode = url_mode(&request.url)?;
    let host = request
        .url
        .host()
        .ok_or_else(|| Error::Url("No host name in the URL".into()))?;
    let port = request
        .url
        .port_or_known_default()
        .ok_or_else(|| Error::Url("No port number in the URL".into()))?;
    let addrs;
    let addr;
    let addrs = match host {
        url::Host::Domain(domain) => {
            addrs = (domain, port).to_socket_addrs()?;
            addrs.as_slice()
        }
        url::Host::Ipv4(ip) => {
            addr = (ip, port).into();
            std::slice::from_ref(&addr)
        }
        url::Host::Ipv6(ip) => {
            addr = (ip, port).into();
            std::slice::from_ref(&addr)
        }
    };
    let mut stream = connect_to_some(addrs, &request.url, mode)?;
    NoDelay::set_nodelay(&mut stream, true)?;
    client_with_config(request, stream, config).map_err(|e| match e {
        HandshakeError::Failure(f) => f,
        HandshakeError::Interrupted(_) => panic!("Bug: blocking handshake not blocked"),
    })
}

/// Connect to the given WebSocket in blocking mode.
///
/// The URL may be either ws:// or wss://.
/// To support wss:// URLs, feature "tls" must be turned on.
///
/// This function "just works" for those who wants a simple blocking solution
/// similar to `std::net::TcpStream`. If you want a non-blocking or other
/// custom stream, call `client` instead.
///
/// This function uses `native_tls` to do TLS. If you want to use other TLS libraries,
/// use `client` instead. There is no need to enable the "tls" feature if you don't call
/// `connect` since it's the only function that uses native_tls.
pub fn connect<'t, Req: Into<Request<'t>>>(
    request: Req,
) -> Result<(WebSocket<AutoStream>, Response)> {
    connect_with_config(request, None)
}

fn connect_to_some(addrs: &[SocketAddr], url: &Url, mode: Mode) -> Result<AutoStream> {
    let domain = url
        .host_str()
        .ok_or_else(|| Error::Url("No host name in the URL".into()))?;
    for addr in addrs {
        debug!("Trying to contact {} at {}...", url, addr);
        if let Ok(raw_stream) = TcpStream::connect(addr) {
            if let Ok(stream) = wrap_stream(raw_stream, domain, mode) {
                return Ok(stream);
            }
        }
    }
    Err(Error::Url(format!("Unable to connect to {}", url).into()))
}

/// Get the mode of the given URL.
///
/// This function may be used to ease the creation of custom TLS streams
/// in non-blocking algorithmss or for use with TLS libraries other than `native_tls`.
pub fn url_mode(url: &Url) -> Result<Mode> {
    match url.scheme() {
        "ws" => Ok(Mode::Plain),
        "wss" => Ok(Mode::Tls),
        _ => Err(Error::Url("URL scheme not supported".into())),
    }
}

/// Do the client handshake over the given stream given a web socket configuration. Passing `None`
/// as configuration is equal to calling `client()` function.
///
/// Use this function if you need a nonblocking handshake support or if you
/// want to use a custom stream like `mio::tcp::TcpStream` or `openssl::ssl::SslStream`.
/// Any stream supporting `Read + Write` will do.
pub fn client_with_config<'t, Stream, Req>(
    request: Req,
    stream: Stream,
    config: Option<WebSocketConfig>,
) -> StdResult<(WebSocket<Stream>, Response), HandshakeError<ClientHandshake<Stream>>>
where
    Stream: Read + Write,
    Req: Into<Request<'t>>,
{
    ClientHandshake::start(stream, request.into(), config).handshake()
}

/// Do the client handshake over the given stream.
///
/// Use this function if you need a nonblocking handshake support or if you
/// want to use a custom stream like `mio::tcp::TcpStream` or `openssl::ssl::SslStream`.
/// Any stream supporting `Read + Write` will do.
pub fn client<'t, Stream, Req>(
    request: Req,
    stream: Stream,
) -> StdResult<(WebSocket<Stream>, Response), HandshakeError<ClientHandshake<Stream>>>
where
    Stream: Read + Write,
    Req: Into<Request<'t>>,
{
    client_with_config(request, stream, None)
}
