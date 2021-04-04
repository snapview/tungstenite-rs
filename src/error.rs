//! Error handling.

mod capacity_error;
mod protocol_error;
mod tls_error;
mod url_error;

use crate::protocol::Message;
pub use capacity_error::CapacityError;
use http::Response;
pub use protocol_error::ProtocolError;
use std::{fmt, io, result, str, string};
pub use tls_error::TlsError;
pub use url_error::UrlError;

/// Result type of all Tungstenite library calls.
pub type Result<T> = result::Result<T, Error>;

/// Possible WebSocket errors.
pub enum Error {
    /// WebSocket connection closed normally. This informs you of the close.
    /// It's not an error as such and nothing wrong happened.
    ///
    /// This is returned as soon as the close handshake is finished (we have both sent and
    /// received a close frame) on the server end and as soon as the server has closed the
    /// underlying connection if this endpoint is a client.
    ///
    /// Thus when you receive this, it is safe to drop the underlying connection.
    ///
    /// Receiving this error means that the WebSocket object is not usable anymore and the
    /// only meaningful action with it is dropping it.
    ConnectionClosed,
    /// Trying to work with already closed connection.
    ///
    /// Trying to read or write after receiving `ConnectionClosed` causes this.
    ///
    /// As opposed to `ConnectionClosed`, this indicates your code tries to operate on the
    /// connection when it really shouldn't anymore, so this really indicates a programmer
    /// error on your part.
    AlreadyClosed,
    /// Input-output error. Apart from WouldBlock, these are generally errors with the
    /// underlying connection and you should probably consider them fatal.
    Io(io::Error),
    /// TLS error.
    ///
    /// Note that this error variant is enabled unconditionally even if no TLS feature is enabled,
    /// to provide a feature-agnostic API surface.
    Tls(TlsError),
    /// - When reading: buffer capacity exhausted.
    /// - When writing: your message is bigger than the configured max message size
    ///   (64MB by default).
    Capacity(CapacityError),
    /// Protocol violation.
    Protocol(ProtocolError),
    /// Message send queue full.
    SendQueueFull(Message),
    /// UTF coding error.
    Utf8,
    /// Invalid URL.
    Url(UrlError),
    /// HTTP error.
    Http(Response<Option<String>>),
    /// HTTP format error.
    HttpFormat(http::Error),
}

impl From<io::Error> for Error {
    #[inline]
    fn from(from: io::Error) -> Self {
        Self::Io(from)
    }
}

impl From<TlsError> for Error {
    #[inline]
    fn from(from: TlsError) -> Self {
        Self::Tls(from)
    }
}

impl From<CapacityError> for Error {
    #[inline]
    fn from(from: CapacityError) -> Self {
        Self::Capacity(from)
    }
}

impl From<ProtocolError> for Error {
    #[inline]
    fn from(from: ProtocolError) -> Self {
        Self::Protocol(from)
    }
}

impl From<Message> for Error {
    #[inline]
    fn from(from: Message) -> Self {
        Self::SendQueueFull(from)
    }
}

impl From<UrlError> for Error {
    #[inline]
    fn from(from: UrlError) -> Self {
        Self::Url(from)
    }
}

impl From<Response<Option<String>>> for Error {
    #[inline]
    fn from(from: Response<Option<String>>) -> Self {
        Self::Http(from)
    }
}

impl From<http::Error> for Error {
    #[inline]
    fn from(from: http::Error) -> Self {
        Self::HttpFormat(from)
    }
}

impl From<str::Utf8Error> for Error {
    fn from(_: str::Utf8Error) -> Self {
        Error::Utf8
    }
}

impl From<string::FromUtf8Error> for Error {
    fn from(_: string::FromUtf8Error) -> Self {
        Error::Utf8
    }
}

impl From<http::header::InvalidHeaderValue> for Error {
    fn from(err: http::header::InvalidHeaderValue) -> Self {
        Error::HttpFormat(err.into())
    }
}

impl From<http::header::InvalidHeaderName> for Error {
    fn from(err: http::header::InvalidHeaderName) -> Self {
        Error::HttpFormat(err.into())
    }
}

impl From<http::header::ToStrError> for Error {
    fn from(_: http::header::ToStrError) -> Self {
        Error::Utf8
    }
}

impl From<http::uri::InvalidUri> for Error {
    fn from(err: http::uri::InvalidUri) -> Self {
        Error::HttpFormat(err.into())
    }
}

impl From<http::status::InvalidStatusCode> for Error {
    fn from(err: http::status::InvalidStatusCode) -> Self {
        Error::HttpFormat(err.into())
    }
}

impl From<httparse::Error> for Error {
    fn from(err: httparse::Error) -> Self {
        match err {
            httparse::Error::TooManyHeaders => Error::Capacity(CapacityError::TooManyHeaders),
            e => Error::Protocol(ProtocolError::HttparseError(e)),
        }
    }
}

impl fmt::Debug for Error {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::ConnectionClosed => write!(f, "Connection closed normally"),
            Self::AlreadyClosed => write!(f, "Trying to work with closed connection"),
            Self::Io(ref elem) => write!(f, "IO error: {}", elem),
            Self::Tls(ref elem) => write!(f, "TLS error: {}", elem),
            Self::Capacity(ref elem) => write!(f, "Space limit exceeded: {}", elem),
            Self::Protocol(ref elem) => write!(f, "WebSocket protocol error: {}", elem),
            Self::SendQueueFull(ref elem) => write!(f, "Send queue is full: {}", elem),
            Self::Utf8 => write!(f, "UTF-8 encoding error"),
            Self::Url(ref elem) => write!(f, "URL error: {}", elem),
            Self::Http(ref elem) => write!(f, "HTTP error: {:?}", elem),
            Self::HttpFormat(ref elem) => write!(f, "HTTP format error: {}", elem),
        }
    }
}

impl fmt::Display for Error {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

impl std::error::Error for Error {}
