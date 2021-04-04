use std::fmt;

/// Indicates the specific type/cause of URL error.
#[derive(PartialEq, Eq)]
pub enum UrlError {
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

impl fmt::Debug for UrlError {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::TlsFeatureNotEnabled => write!(f, "TLS support not compiled in"),
            Self::NoHostName => write!(f, "No host name in the URL"),
            Self::UnableToConnect(ref elem) => write!(f, "Unable to connect to {}", elem),
            Self::UnsupportedUrlScheme => write!(f, "URL scheme not supported"),
            Self::EmptyHostName => write!(f, "URL contains empty host name"),
            Self::NoPathOrQuery => write!(f, "No path/query in URL"),
        }
    }
}

impl fmt::Display for UrlError {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

impl std::error::Error for UrlError {}
