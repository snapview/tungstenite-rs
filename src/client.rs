//! Methods to connect to an WebSocket as a client.

use std::net::{TcpStream, SocketAddr, ToSocketAddrs};
use std::result::Result as StdResult;
use std::io::{Read, Write};

use url::Url;

#[cfg(feature="tls")]
use native_tls::{TlsStream, TlsConnector, HandshakeError as TlsHandshakeError};

use protocol::WebSocket;
use handshake::HandshakeError;
use handshake::client::{ClientHandshake, Request};
use stream::Mode;
use error::{Error, Result};

#[cfg(feature="tls")]
use stream::Stream as StreamSwitcher;

#[cfg(feature="tls")]
pub type AutoStream = StreamSwitcher<TcpStream, TlsStream<TcpStream>>;
#[cfg(not(feature="tls"))]
pub type AutoStream = TcpStream;

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
pub fn connect(url: Url) -> Result<WebSocket<AutoStream>> {
    let mode = url_mode(&url)?;
    let addrs = url.to_socket_addrs()?;
    let stream = connect_to_some(addrs, &url, mode)?;
    client(url.clone(), stream)
        .map_err(|e| match e {
            HandshakeError::Failure(f) => f,
            HandshakeError::Interrupted(_) => panic!("Bug: blocking handshake not blocked"),
        })
}

#[cfg(feature="tls")]
fn wrap_stream(stream: TcpStream, domain: &str, mode: Mode) -> Result<AutoStream> {
    match mode {
        Mode::Plain => Ok(StreamSwitcher::Plain(stream)),
        Mode::Tls => {
            let connector = TlsConnector::builder()?.build()?;
            connector.connect(domain, stream)
                .map_err(|e| match e {
                    TlsHandshakeError::Failure(f) => f.into(),
                    TlsHandshakeError::Interrupted(_) => panic!("Bug: TLS handshake not blocked"),
                })
                .map(|s| StreamSwitcher::Tls(s))
        }
    }
}

#[cfg(not(feature="tls"))]
fn wrap_stream(stream: TcpStream, _domain: &str, mode: Mode) -> Result<AutoStream> {
    match mode {
        Mode::Plain => Ok(stream),
        Mode::Tls => Err(Error::Url("TLS support not compiled in.".into())),
    }
}

fn connect_to_some<A>(addrs: A, url: &Url, mode: Mode) -> Result<AutoStream>
    where A: Iterator<Item=SocketAddr>
{
    let domain = url.host_str().ok_or_else(|| Error::Url("No host name in the URL".into()))?;
    for addr in addrs {
        debug!("Trying to contact {} at {}...", url, addr);
        if let Ok(raw_stream) = TcpStream::connect(addr) {
            if let Ok(stream) = wrap_stream(raw_stream, domain, mode) {
                return Ok(stream)
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
        _ => Err(Error::Url("URL scheme not supported".into()))
    }
}

/// Do the client handshake over the given stream.
///
/// Use this function if you need a nonblocking handshake support or if you
/// want to use a custom stream like `mio::tcp::TcpStream` or `openssl::ssl::SslStream`.
/// Any stream supporting `Read + Write` will do.
pub fn client<Stream: Read + Write>(url: Url, stream: Stream)
    -> StdResult<WebSocket<Stream>, HandshakeError<Stream, ClientHandshake>>
{
    let request = Request { url: url };
    ClientHandshake::start(stream, request).handshake()
}
