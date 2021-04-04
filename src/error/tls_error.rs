use std::fmt;

/// TLS errors.
///
/// Note that even if you enable only the rustls-based TLS support, the error at runtime could still
/// be `Native`, as another crate in the dependency graph may enable native TLS support.
#[allow(missing_copy_implementations)]
#[non_exhaustive]
pub enum TlsError {
    /// Native TLS error.
    #[cfg(feature = "native-tls")]
    Native(native_tls_crate::Error),
    /// Rustls error.
    #[cfg(feature = "rustls-tls")]
    Rustls(rustls::TLSError),
    /// DNS name resolution error.
    #[cfg(feature = "rustls-tls")]
    Dns(webpki::InvalidDNSNameError),
}

impl fmt::Debug for TlsError {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            #[cfg(feature = "native-tls")]
            Self::Native(ref elem) => write!(f, "native-tls error: {}", elem),
            #[cfg(feature = "rustls-tls")]
            Self::Rustls(ref elem) => write!(f, "rustls error: {}", elem),
            #[cfg(feature = "rustls-tls")]
            Self::Dns(ref elem) => write!(f, "Invalid DNS name: {}", elem),
        }
    }
}

#[cfg(feature = "native-tls")]
impl From<native_tls_crate::Error> for TlsError {
    #[inline]
    fn from(from: native_tls_crate::Error) -> Self {
        Self::Native(from)
    }
}

#[cfg(feature = "rustls-tls")]
impl From<rustls::TLSError> for TlsError {
    #[inline]
    fn from(from: rustls::TLSError) -> Self {
        Self::Rustls(from)
    }
}

#[cfg(feature = "rustls-tls")]
impl From<webpki::InvalidDNSNameError> for TlsError {
    #[inline]
    fn from(from: webpki::InvalidDNSNameError) -> Self {
        Self::Dns(from)
    }
}

impl fmt::Display for TlsError {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

impl std::error::Error for TlsError {}
