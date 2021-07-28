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

pub use http;

pub mod buffer;
pub mod client;
pub mod error;
pub mod handshake;
pub mod protocol;
mod server;
pub mod stream;
#[cfg(any(feature = "native-tls", feature = "__rustls-tls"))]
mod tls;
pub mod util;

const READ_BUFFER_CHUNK_SIZE: usize = 4096;
type ReadBuffer = buffer::ReadBuffer<READ_BUFFER_CHUNK_SIZE>;

pub use crate::{
    client::{client, connect},
    error::{Error, Result},
    handshake::{client::ClientHandshake, server::ServerHandshake, HandshakeError},
    protocol::{Message, WebSocket},
    server::{accept, accept_hdr, accept_hdr_with_config, accept_with_config},
};

#[cfg(any(feature = "native-tls", feature = "__rustls-tls"))]
pub use tls::{client_tls, client_tls_with_config, Connector};
