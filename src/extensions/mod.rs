//! WebSocket extensions.
// Only `permessage-deflate` is supported at the moment.

#[cfg(feature = "deflate")]
mod compression;
#[cfg(feature = "deflate")]
use compression::deflate::DeflateContext;
#[cfg(feature = "deflate")]
pub use compression::deflate::{DeflateConfig, DeflateError};

/// Container for configured extensions.
#[derive(Debug, Default)]
#[allow(missing_copy_implementations)]
pub struct Extensions {
    // Per-Message Compression. Only `permessage-deflate` is supported.
    #[cfg(feature = "deflate")]
    pub(crate) compression: Option<DeflateContext>,
}
