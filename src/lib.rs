//! Lightweight, flexible WebSockets for Rust.
#![deny(
    missing_docs,
    missing_copy_implementations,
    missing_debug_implementations,
    trivial_casts,
    trivial_numeric_casts,
    unstable_features,
    unused_must_use,
    unused_mut,
    unused_imports,
    unused_import_braces
)]

#[cfg(feature = "handshake")]
pub use http;

pub mod buffer;
#[cfg(feature = "handshake")]
pub mod client;
pub mod error;
#[cfg(feature = "handshake")]
pub mod handshake;
pub mod protocol;
#[cfg(feature = "handshake")]
mod server;
pub mod stream;
#[cfg(any(feature = "native-tls", feature = "__rustls-tls"))]
mod tls;
pub mod util;

const READ_BUFFER_CHUNK_SIZE: usize = 4096;
type ReadBuffer = buffer::ReadBuffer<READ_BUFFER_CHUNK_SIZE>;

pub use crate::{
    error::{Error, Result},
    protocol::{Message, WebSocket},
};

#[cfg(feature = "handshake")]
pub use crate::{
    client::{client, connect, connect_to_raw_stream},
    handshake::{client::ClientHandshake, server::ServerHandshake, HandshakeError},
    server::{accept, accept_hdr, accept_hdr_with_config, accept_with_config},
};

#[cfg(any(feature = "native-tls", feature = "__rustls-tls"))]
pub use tls::{client_tls, client_tls_with_config, Connector};
