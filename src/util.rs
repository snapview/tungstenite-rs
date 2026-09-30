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
pub(crate) fn strip_ipv6_brackets(host: &str) -> &str {
    host.strip_prefix('[').and_then(|host| host.strip_suffix(']')).unwrap_or(host)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bracketed_ipv6_literal_is_unbracketed() {
        assert_eq!(strip_ipv6_brackets("[::1]"), "::1");
        assert_eq!(strip_ipv6_brackets("[2001:db8::1]"), "2001:db8::1");
    }

    #[test]
    fn an_ipv4_mapped_ipv6_literal_is_unbracketed_the_same_way() {
        // This function only strips the delimiters; it never inspects
        // what's between them, so an embedded dotted-quad is no
        // different from any other IPv6 text form.
        assert_eq!(strip_ipv6_brackets("[::ffff:192.168.1.1]"), "::ffff:192.168.1.1");
    }

    #[test]
    fn a_zone_id_is_carried_through_unbracketed_but_not_made_valid() {
        // RFC 6874 zone IDs (link-local scope, e.g. "%eth0") are stripped
        // of their brackets the same as any other host, but the result
        // still isn't accepted by Ipv6Addr::from_str or a TLS server
        // name. Neither has ever supported scope IDs, bracketed or not,
        // and this function doesn't change that. Not a problem worth
        // solving here: a certificate can't attest to a link-local scope
        // in the first place, so no real TLS server needs one.
        assert_eq!(strip_ipv6_brackets("[fe80::1%25eth0]"), "fe80::1%25eth0");
        assert!("fe80::1%25eth0".parse::<std::net::Ipv6Addr>().is_err());
    }

    #[test]
    fn an_ipv4_address_is_unaffected() {
        assert_eq!(strip_ipv6_brackets("192.168.1.1"), "192.168.1.1");
    }

    #[test]
    fn a_dns_name_is_unaffected() {
        assert_eq!(strip_ipv6_brackets("example.com"), "example.com");
    }

    #[test]
    fn an_unmatched_bracket_is_left_alone_rather_than_mis_sliced() {
        // Only strips a matching pair; a malformed host is left for
        // whatever reads it next (DNS resolution, a TLS server name) to
        // reject on its own terms.
        assert_eq!(strip_ipv6_brackets("[::1"), "[::1");
        assert_eq!(strip_ipv6_brackets("::1]"), "::1]");
    }
}
