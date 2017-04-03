//! Error handling.

use std::borrow::{Borrow, Cow};

use std::error::Error as ErrorTrait;
use std::fmt;
use std::io;
use std::result;
use std::str;
use std::string;

use httparse;

#[cfg(feature="tls")]
pub mod tls {
    pub use native_tls::Error;
}

pub type Result<T> = result::Result<T, Error>;

/// Possible WebSocket errors
#[derive(Debug)]
pub enum Error {
    /// WebSocket connection closed (normally)
    ConnectionClosed,
    /// Input-output error
    Io(io::Error),
    #[cfg(feature="tls")]
    /// TLS error
    Tls(tls::Error),
    /// Buffer capacity exhausted
    Capacity(Cow<'static, str>),
    /// Protocol violation
    Protocol(Cow<'static, str>),
    /// UTF coding error
    Utf8,
    /// Invlid URL.
    Url(Cow<'static, str>),
    /// HTTP error.
    Http(u16),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            Error::ConnectionClosed => write!(f, "Connection closed"),
            Error::Io(ref err) => write!(f, "IO error: {}", err),
            #[cfg(feature="tls")]
            Error::Tls(ref err) => write!(f, "TLS error: {}", err),
            Error::Capacity(ref msg) => write!(f, "Space limit exceeded: {}", msg),
            Error::Protocol(ref msg) => write!(f, "WebSocket protocol error: {}", msg),
            Error::Utf8 => write!(f, "UTF-8 encoding error"),
            Error::Url(ref msg) => write!(f, "URL error: {}", msg),
            Error::Http(code) => write!(f, "HTTP code: {}", code),
        }
    }
}

impl ErrorTrait for Error {
    fn description(&self) -> &str {
        match *self {
            Error::ConnectionClosed => "",
            Error::Io(ref err) => err.description(),
            #[cfg(feature="tls")]
            Error::Tls(ref err) => err.description(),
            Error::Capacity(ref msg) => msg.borrow(),
            Error::Protocol(ref msg) => msg.borrow(),
            Error::Utf8 => "",
            Error::Url(ref msg) => msg.borrow(),
            Error::Http(_) => "",
        }
    }
}

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

#[cfg(feature="tls")]
impl From<tls::Error> for Error {
    fn from(err: tls::Error) -> Self {
        Error::Tls(err)
    }
}

impl From<httparse::Error> for Error {
    fn from(err: httparse::Error) -> Self {
        match err {
            httparse::Error::TooManyHeaders => Error::Capacity("Too many headers".into()),
            e => Error::Protocol(Cow::Owned(e.description().into())),
        }
    }
}
