//! Helper traits to ease non-blocking handling.

use std::{
    io::{Error as IoError, ErrorKind as IoErrorKind},
    result::Result as StdResult,
};

use crate::error::Error;

/// Non-blocking IO handling.
pub trait NonBlockingError: Sized {
    /// Convert WouldBlock to None and don't touch other errors.
    fn into_non_blocking(self) -> Option<Self>;
}

impl NonBlockingError for IoError {
    fn into_non_blocking(self) -> Option<Self> {
        match self.kind() {
            IoErrorKind::WouldBlock => None,
            _ => Some(self),
        }
    }
}

impl NonBlockingError for Error {
    fn into_non_blocking(self) -> Option<Self> {
        match self {
            Error::Io(e) => e.into_non_blocking().map(Into::into),
            x => Some(x),
        }
    }
}

/// Non-blocking IO wrapper.
///
/// This trait is implemented for `Result<T, E: NonBlockingError>`.
pub trait NonBlockingResult {
    /// Type of the converted result: `Result<Option<T>, E>`
    type Result;
    /// Perform the non-block conversion.
    fn no_block(self) -> Self::Result;
}

impl<T, E> NonBlockingResult for StdResult<T, E>
where
    E: NonBlockingError,
{
    type Result = StdResult<Option<T>, E>;
    fn no_block(self) -> Self::Result {
        match self {
            Ok(x) => Ok(Some(x)),
            Err(e) => match e.into_non_blocking() {
                Some(e) => Err(e),
                None => Ok(None),
            },
        }
    }
}

/// Strips the brackets from a bracketed IPv6 literal in a URI's authority
/// (RFC 3986), e.g. `"[::1]"` becomes `"::1"`. Returns `host` unchanged if
/// it isn't bracketed. Neither a socket address lookup nor a TLS server
/// name accepts the bracketed form, so both `client::connect` and the TLS
/// connectors need this.
#[cfg(feature = "handshake")]
pub(crate) fn strip_ipv6_brackets(host: &str) -> &str {
    host.strip_prefix('[').and_then(|host| host.strip_suffix(']')).unwrap_or(host)
}

#[cfg(all(test, feature = "handshake"))]
mod tests {
    use super::*;

    #[test]
    fn strip_ipv6_brackets_cases() {
        let cases = [
            // Bracketed IPv6 literals are unbracketed.
            ("[::1]", "::1"),
            ("[2001:db8::1]", "2001:db8::1"),
            // Only the delimiters are stripped, never the content, so an
            // embedded dotted-quad is no different from any other IPv6 text.
            ("[::ffff:192.168.1.1]", "::ffff:192.168.1.1"),
            // RFC 6874 zone IDs are unbracketed like any other host, but the
            // result is still not accepted by `Ipv6Addr::from_str` or as a
            // TLS server name. This function does not change that.
            ("[fe80::1%25eth0]", "fe80::1%25eth0"),
            // Hosts that are not bracketed are unaffected.
            ("192.168.1.1", "192.168.1.1"),
            ("example.com", "example.com"),
            // Only a matching pair is stripped; a malformed host is left for
            // whatever reads it next (DNS resolution, TLS server name) to reject.
            ("[::1", "[::1"),
            ("::1]", "::1]"),
        ];

        for (host, expected) in cases {
            assert_eq!(strip_ipv6_brackets(host), expected, "host: {host:?}");
        }

        assert!("fe80::1%25eth0".parse::<std::net::Ipv6Addr>().is_err());
    }
}
