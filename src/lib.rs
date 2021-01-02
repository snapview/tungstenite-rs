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

pub mod client;
pub mod error;
pub mod handshake;
pub mod protocol;
pub mod server;
pub mod stream;
pub mod util;

pub use crate::{
    client::{client, connect},
    error::{Error, Result},
    handshake::{client::ClientHandshake, server::ServerHandshake, HandshakeError},
    protocol::{Message, WebSocket},
    server::{accept, accept_hdr},
};

#[cfg(all(feature = "native-tls", feature = "rustls-tls"))]
compile_error!("either \"native-tls\" or \"rustls-tls\" can be enabled, but not both.");
