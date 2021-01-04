//! Error handling.

use std::{error::Error as ErrorTrait, fmt, io, result, str, string};

use crate::protocol::{frame::coding::Data, Message};
use http::Response;

#[cfg(feature = "tls")]
pub mod tls {
    //! TLS error wrapper module, feature-gated.
    pub use native_tls::Error;
}

/// Result type of all Tungstenite library calls.
pub type Result<T> = result::Result<T, Error>;

/// Possible WebSocket errors.
#[derive(Debug)]
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
    #[cfg(feature = "tls")]
    /// TLS error.
    Tls(tls::Error),
    /// - When reading: buffer capacity exhausted.
    /// - When writing: your message is bigger than the configured max message size
    ///   (64MB by default).
    Capacity(CapacityErrorType),
    /// Protocol violation.
    Protocol(ProtocolErrorType),
    /// Message send queue full.
    SendQueueFull(Message),
    /// UTF coding error
    Utf8,
    /// Invalid URL.
    Url(UrlErrorType),
    /// HTTP error.
    Http(Response<Option<String>>),
    /// HTTP format error.
    HttpFormat(http::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            Error::ConnectionClosed => write!(f, "Connection closed normally"),
            Error::AlreadyClosed => write!(f, "Trying to work with closed connection"),
            Error::Io(ref err) => write!(f, "IO error: {}", err),
            #[cfg(feature = "tls")]
            Error::Tls(ref err) => write!(f, "TLS error: {}", err),
            Error::Capacity(ref msg) => write!(f, "Space limit exceeded: {}", msg),
            Error::Protocol(ref msg) => write!(f, "WebSocket protocol error: {}", msg),
            Error::SendQueueFull(_) => write!(f, "Send queue is full"),
            Error::Utf8 => write!(f, "UTF-8 encoding error"),
            Error::Url(ref msg) => write!(f, "URL error: {}", msg),
            Error::Http(ref code) => write!(f, "HTTP error: {}", code.status()),
            Error::HttpFormat(ref err) => write!(f, "HTTP format error: {}", err),
        }
    }
}

impl ErrorTrait for Error {}

impl From<io::Error> for Error {
    fn from(err: io::Error) -> Self {
        Error::Io(err)
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

impl From<http::Error> for Error {
    fn from(err: http::Error) -> Self {
        Error::HttpFormat(err)
    }
}

#[cfg(feature = "tls")]
impl From<tls::Error> for Error {
    fn from(err: tls::Error) -> Self {
        Error::Tls(err)
    }
}

impl From<httparse::Error> for Error {
    fn from(err: httparse::Error) -> Self {
        match err {
            httparse::Error::TooManyHeaders => Error::Capacity(CapacityErrorType::TooManyHeaders),
            e => Error::Protocol(ProtocolErrorType::HttparseError(e)),
        }
    }
}

/// Indicates the specific type/cause of a capacity error.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum CapacityErrorType {
    /// Too many headers provided (see [`httparse::Error::TooManyHeaders`]).
    TooManyHeaders,
    /// Received header is too long.
    HeaderTooLong,
    /// Message is bigger than the maximum allowed size.
    MessageTooLong {
        /// The size of the message.
        size: usize,
        /// The maximum allowed message size.
        max_size: usize,
    },
    /// TCP buffer is full.
    TcpBufferFull,
}

impl fmt::Display for CapacityErrorType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            CapacityErrorType::TooManyHeaders => write!(f, "Too many headers"),
            CapacityErrorType::HeaderTooLong => write!(f, "Header too long"),
            CapacityErrorType::MessageTooLong { size, max_size } => {
                write!(f, "Message too long: {} > {}", size, max_size)
            }
            CapacityErrorType::TcpBufferFull => write!(f, "Incoming TCP buffer is full"),
        }
    }
}

/// Indicates the specific type/cause of a protocol error.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum ProtocolErrorType {
    /// Use of the wrong HTTP method (the WebSocket protocol requires the GET method be used).
    WrongHttpMethod,
    /// Wrong HTTP version used (the WebSocket protocol requires version 1.1 or higher).
    WrongHttpVersion,
    /// Missing `Connection: upgrade` HTTP header.
    MissingConnectionUpgradeHeader,
    /// Missing `Upgrade: websocket` HTTP header.
    MissingUpgradeWebSocketHeader,
    /// Missing `Sec-WebSocket-Version: 13` HTTP header.
    MissingSecWebSocketVersionHeader,
    /// Missing `Sec-WebSocket-Key` HTTP header.
    MissingSecWebSocketKey,
    /// The `Sec-WebSocket-Accept` header is either not present or does not specify the correct key value.
    SecWebSocketAcceptKeyMismatch,
    /// Garbage data encountered after client request.
    JunkAfterRequest,
    /// Custom responses must be unsuccessful.
    CustomResponseSuccessful,
    /// No more data while still performing handshake.
    HandshakeIncomplete,
    /// Wrapper around a [`httparse::Error`] value.
    HttparseError(httparse::Error),
    /// Not allowed to send after having sent a closing frame.
    SendAfterClosing,
    /// Remote sent data after sending a closing frame.
    ReceivedAfterClosing,
    /// Reserved bits in frame header are non-zero.
    NonZeroReservedBits,
    /// The server must close the connection when an unmasked frame is received.
    UnmaskedFrameFromClient,
    /// The client must close the connection when a masked frame is received.
    MaskedFrameFromServer,
    /// Control frames must not be fragmented.
    FragmentedControlFrame,
    /// Control frames must have a payload of 125 bytes or less.
    ControlFrameTooBig,
    /// Type of control frame not recognised.
    UnknownControlFrameType(u8),
    /// Type of data frame not recognised.
    UnknownDataFrameType(u8),
    /// Received a continue frame despite there being nothing to continue.
    UnexpectedContinueFrame,
    /// Received data while waiting for more fragments.
    ExpectedFragment(Data),
    /// Connection closed without performing the closing handshake.
    ResetWithoutClosingHandshake,
    /// Encountered an invalid opcode.
    InvalidOpcode(u8),
    /// The payload for the closing frame is invalid.
    InvalidCloseSequence,
}

impl fmt::Display for ProtocolErrorType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            ProtocolErrorType::WrongHttpMethod => {
                write!(f, "Unsupported HTTP method used, only GET is allowed")
            }
            ProtocolErrorType::WrongHttpVersion => write!(f, "HTTP version must be 1.1 or higher"),
            ProtocolErrorType::MissingConnectionUpgradeHeader => {
                write!(f, "No \"Connection: upgrade\" header")
            }
            ProtocolErrorType::MissingUpgradeWebSocketHeader => {
                write!(f, "No \"Upgrade: websocket\" header")
            }
            ProtocolErrorType::MissingSecWebSocketVersionHeader => {
                write!(f, "No \"Sec-WebSocket-Version: 13\" header")
            }
            ProtocolErrorType::MissingSecWebSocketKey => {
                write!(f, "No \"Sec-WebSocket-Key\" header")
            }
            ProtocolErrorType::SecWebSocketAcceptKeyMismatch => {
                write!(f, "Key mismatch in \"Sec-WebSocket-Accept\" header")
            }
            ProtocolErrorType::JunkAfterRequest => write!(f, "Junk after client request"),
            ProtocolErrorType::CustomResponseSuccessful => {
                write!(f, "Custom response must not be successful")
            }
            ProtocolErrorType::HandshakeIncomplete => write!(f, "Handshake not finished"),
            ProtocolErrorType::HttparseError(e) => write!(f, "httparse error: {}", e),
            ProtocolErrorType::SendAfterClosing => {
                write!(f, "Sending after closing is not allowed")
            }
            ProtocolErrorType::ReceivedAfterClosing => write!(f, "Remote sent after having closed"),
            ProtocolErrorType::NonZeroReservedBits => write!(f, "Reserved bits are non-zero"),
            ProtocolErrorType::UnmaskedFrameFromClient => {
                write!(f, "Received an unmasked frame from client")
            }
            ProtocolErrorType::MaskedFrameFromServer => {
                write!(f, "Received a masked frame from server")
            }
            ProtocolErrorType::FragmentedControlFrame => write!(f, "Fragmented control frame"),
            ProtocolErrorType::ControlFrameTooBig => {
                write!(f, "Control frame too big (payload must be 125 bytes or less)")
            }
            ProtocolErrorType::UnknownControlFrameType(i) => {
                write!(f, "Unknown control frame type: {}", i)
            }
            ProtocolErrorType::UnknownDataFrameType(i) => {
                write!(f, "Unknown data frame type: {}", i)
            }
            ProtocolErrorType::UnexpectedContinueFrame => {
                write!(f, "Continue frame but nothing to continue")
            }
            ProtocolErrorType::ExpectedFragment(c) => {
                write!(f, "While waiting for more fragments received: {}", c)
            }
            ProtocolErrorType::ResetWithoutClosingHandshake => {
                write!(f, "Connection reset without closing handshake")
            }
            ProtocolErrorType::InvalidOpcode(opcode) => {
                write!(f, "Encountered invalid opcode: {}", opcode)
            }
            ProtocolErrorType::InvalidCloseSequence => write!(f, "Invalid close sequence"),
        }
    }
}

/// Indicates the specific type/cause of URL error.
#[derive(Debug, PartialEq, Eq)]
pub enum UrlErrorType {
    /// TLS is used despite not being compiled with the TLS feature enabled.
    TlsFeatureNotEnabled,
    /// The URL does not include a host name.
    NoHostName,
    /// Failed to connect with this URL.
    UnableToConnect(String),
    /// Unsupported URL scheme used (only `ws://` or `wss://` may be used).
    UnsupportedUrlScheme,
    /// The URL host name, though included, is empty.
    EmptyHostName,
    /// The URL does not include a path/query.
    NoPathOrQuery,
}

impl fmt::Display for UrlErrorType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            UrlErrorType::TlsFeatureNotEnabled => write!(f, "TLS support not compiled in"),
            UrlErrorType::NoHostName => write!(f, "No host name in the URL"),
            UrlErrorType::UnableToConnect(uri) => write!(f, "Unable to connect to {}", uri),
            UrlErrorType::UnsupportedUrlScheme => write!(f, "URL scheme not supported"),
            UrlErrorType::EmptyHostName => write!(f, "URL contains empty host name"),
            UrlErrorType::NoPathOrQuery => write!(f, "No path/query in URL"),
        }
    }
}
