//! Error handling.

use std::{io, result, str, string};

use crate::protocol::{frame::coding::Data, Message};
#[cfg(feature = "handshake")]
use http::{header::HeaderName, Response};
use thiserror::Error;

/// Result type of all Tungstenite library calls.
pub type Result<T, E = Error> = result::Result<T, E>;

/// Possible WebSocket errors.
#[derive(Error, Debug)]
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
    #[error("connection closed normally")]
    ConnectionClosed,
    /// Trying to work with already closed connection.
    ///
    /// Trying to read or write after receiving `ConnectionClosed` causes this.
    ///
    /// As opposed to `ConnectionClosed`, this indicates your code tries to operate on the
    /// connection when it really shouldn't anymore, so this really indicates a programmer
    /// error on your part.
    #[error("trying to work with closed connection")]
    AlreadyClosed,
    /// Input-output error. Apart from WouldBlock, these are generally errors with the
    /// underlying connection and you should probably consider them fatal.
    #[error(transparent)]
    Io(#[from] io::Error),
    /// TLS error.
    ///
    /// Note that this error variant is enabled unconditionally even if no TLS feature is enabled,
    /// to provide a feature-agnostic API surface.
    #[error("tls error")]
    Tls(#[from] TlsError),
    /// - When reading: buffer capacity exhausted.
    /// - When writing: your message is bigger than the configured max message size
    ///   (64MB by default).
    #[error("space limit exceeded")]
    Capacity(#[from] CapacityError),
    /// Protocol violation.
    #[error("websocket protocol violated")]
    Protocol(#[from] ProtocolError),
    /// Message write buffer is full.
    #[error("write buffer is full")]
    WriteBufferFull(Message),
    /// UTF coding error.
    #[error("utf-8 encoding error")]
    Utf8,
    /// Attack attempt detected.
    #[error("attack attempt detected")]
    AttackAttempt,
    /// Invalid URL.
    #[error("invalid url")]
    Url(#[from] UrlError),
    /// HTTP error.
    #[error("http error: {}", .0.status())]
    #[cfg(feature = "handshake")]
    Http(Response<Option<Vec<u8>>>),
    /// HTTP format error.
    #[error("http format error")]
    #[cfg(feature = "handshake")]
    HttpFormat(#[from] http::Error),
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

#[cfg(feature = "handshake")]
impl From<http::header::InvalidHeaderValue> for Error {
    fn from(err: http::header::InvalidHeaderValue) -> Self {
        Error::HttpFormat(err.into())
    }
}

#[cfg(feature = "handshake")]
impl From<http::header::InvalidHeaderName> for Error {
    fn from(err: http::header::InvalidHeaderName) -> Self {
        Error::HttpFormat(err.into())
    }
}

#[cfg(feature = "handshake")]
impl From<http::header::ToStrError> for Error {
    fn from(_: http::header::ToStrError) -> Self {
        Error::Utf8
    }
}

#[cfg(feature = "handshake")]
impl From<http::uri::InvalidUri> for Error {
    fn from(err: http::uri::InvalidUri) -> Self {
        Error::HttpFormat(err.into())
    }
}

#[cfg(feature = "handshake")]
impl From<http::status::InvalidStatusCode> for Error {
    fn from(err: http::status::InvalidStatusCode) -> Self {
        Error::HttpFormat(err.into())
    }
}

#[cfg(feature = "handshake")]
impl From<httparse::Error> for Error {
    fn from(err: httparse::Error) -> Self {
        match err {
            httparse::Error::TooManyHeaders => Error::Capacity(CapacityError::TooManyHeaders),
            e => Error::Protocol(ProtocolError::HttparseError(e)),
        }
    }
}

/// Indicates the specific type/cause of a capacity error.
#[derive(Error, Debug, PartialEq, Eq, Clone, Copy)]
pub enum CapacityError {
    /// Too many headers provided (see [`httparse::Error::TooManyHeaders`]).
    #[error("too many headers")]
    TooManyHeaders,
    /// Received header is too long.
    /// Message is bigger than the maximum allowed size.
    #[error("message too long: {size} > {max_size}")]
    MessageTooLong {
        /// The size of the message.
        size: usize,
        /// The maximum allowed message size.
        max_size: usize,
    },
}

/// Indicates the specific type/cause of a subprotocol header error.
#[derive(Error, Clone, PartialEq, Eq, Debug, Copy)]
pub enum SubProtocolError {
    /// The server sent a subprotocol to a client handshake request but none was requested
    #[error("server sent a subprotocol but none was requested")]
    ServerSentSubProtocolNoneRequested,

    /// The server sent an invalid subprotocol to a client handhshake request
    #[error("server sent an invalid subprotocol")]
    InvalidSubProtocol,

    /// The server sent no subprotocol to a client handshake request that requested one or more
    /// subprotocols
    #[error("server sent no subprotocol")]
    NoSubProtocol,
}

/// Indicates the specific type/cause of a protocol error.
#[allow(missing_copy_implementations)]
#[derive(Error, Debug, PartialEq, Eq, Clone)]
pub enum ProtocolError {
    /// Use of the wrong HTTP method (the WebSocket protocol requires the GET method be used).
    #[error("unsupported http method used, only get is allowed")]
    WrongHttpMethod,
    /// Wrong HTTP version used (the WebSocket protocol requires version 1.1 or higher).
    #[error("http version must be 1.1 or higher")]
    WrongHttpVersion,
    /// Missing `Connection: upgrade` HTTP header.
    #[error("missing \"Connection: upgrade\" header")]
    MissingConnectionUpgradeHeader,
    /// Missing `Upgrade: websocket` HTTP header.
    #[error("missing \"Upgrade: websocket\" header")]
    MissingUpgradeWebSocketHeader,
    /// Missing `Sec-WebSocket-Version: 13` HTTP header.
    #[error("missing \"Sec-WebSocket-Version: 13\" header")]
    MissingSecWebSocketVersionHeader,
    /// Missing `Sec-WebSocket-Key` HTTP header.
    #[error("missing \"Sec-WebSocket-Key\" header")]
    MissingSecWebSocketKey,
    /// The `Sec-WebSocket-Accept` header is either not present or does not specify the correct key value.
    #[error("key mismatch in \"Sec-WebSocket-Accept\" header")]
    SecWebSocketAcceptKeyMismatch,
    /// The `Sec-WebSocket-Protocol` header was invalid
    #[error("invalid \"Sec-WebSocket-Protocol\" header")]
    SecWebSocketSubProtocolError(#[from] SubProtocolError),
    /// Garbage data encountered after client request.
    #[error("junk after client request")]
    JunkAfterRequest,
    /// Custom responses must be unsuccessful.
    #[error("custom response must not be successful")]
    CustomResponseSuccessful,
    /// Invalid header is passed. Or the header is missing in the request. Or not present at all. Check the request that you pass.
    #[error("missing, duplicated or incorrect header: {0}")]
    #[cfg(feature = "handshake")]
    InvalidHeader(HeaderName),
    /// No more data while still performing handshake.
    #[error("handshake not finished")]
    HandshakeIncomplete,
    /// Wrapper around a [`httparse::Error`] value.
    #[error("httparse error")]
    #[cfg(feature = "handshake")]
    HttparseError(#[from] httparse::Error),
    /// Not allowed to send after having sent a closing frame.
    #[error("sending after closing is not allowed")]
    SendAfterClosing,
    /// Remote sent data after sending a closing frame.
    #[error("remote sent after having closed")]
    ReceivedAfterClosing,
    /// Reserved bits in frame header are non-zero.
    #[error("reserved bits are non-zero")]
    NonZeroReservedBits,
    /// The server must close the connection when an unmasked frame is received.
    #[error("received an unmasked frame from client")]
    UnmaskedFrameFromClient,
    /// The client must close the connection when a masked frame is received.
    #[error("received a masked frame from server")]
    MaskedFrameFromServer,
    /// Control frames must not be fragmented.
    #[error("fragmented control frame")]
    FragmentedControlFrame,
    /// Control frames must have a payload of 125 bytes or less.
    #[error("control frame too big, payload must be 125 bytes or less")]
    ControlFrameTooBig,
    /// Type of control frame not recognised.
    #[error("unknown control frame type: {0}")]
    UnknownControlFrameType(u8),
    /// Type of data frame not recognised.
    #[error("unknown data frame type: {0}")]
    UnknownDataFrameType(u8),
    /// Received a continue frame despite there being nothing to continue.
    #[error("continue frame but nothing to continue")]
    UnexpectedContinueFrame,
    /// Received data while waiting for more fragments.
    #[error("while waiting for more fragments, received: {0}")]
    ExpectedFragment(Data),
    /// Connection closed without performing the closing handshake.
    #[error("connection reset without closing handshake")]
    ResetWithoutClosingHandshake,
    /// Encountered an invalid opcode.
    #[error("encountered invalid opcode: {0}")]
    InvalidOpcode(u8),
    /// The payload for the closing frame is invalid.
    #[error("invalid close sequence")]
    InvalidCloseSequence,
}

/// Indicates the specific type/cause of URL error.
#[derive(Error, Debug, PartialEq, Eq)]
pub enum UrlError {
    /// TLS is used despite not being compiled with the TLS feature enabled.
    #[error("tls support not compiled in")]
    TlsFeatureNotEnabled,
    /// The URL does not include a host name.
    #[error("no host name in the url")]
    NoHostName,
    /// Failed to connect with this URL.
    #[error("unable to connect to {0}")]
    UnableToConnect(String),
    /// Unsupported URL scheme used (only `ws://` or `wss://` may be used).
    #[error("url scheme not supported")]
    UnsupportedUrlScheme,
    /// The URL host name, though included, is empty.
    #[error("url contains empty host name")]
    EmptyHostName,
    /// The URL does not include a path/query.
    #[error("no path/query in url")]
    NoPathOrQuery,
}

/// TLS errors.
///
/// Note that even if you enable only the rustls-based TLS support, the error at runtime could still
/// be `Native`, as another crate in the dependency graph may enable native TLS support.
#[allow(missing_copy_implementations)]
#[derive(Error, Debug)]
#[non_exhaustive]
pub enum TlsError {
    /// Native TLS error.
    #[cfg(feature = "native-tls")]
    #[error(transparent)]
    Native(#[from] native_tls_crate::Error),
    /// Rustls error.
    #[cfg(feature = "__rustls-tls")]
    #[error(transparent)]
    Rustls(#[from] rustls::Error),
    /// DNS name resolution error.
    #[cfg(feature = "__rustls-tls")]
    #[error("invalid dns name")]
    InvalidDnsName,
}
