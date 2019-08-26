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

pub use crate::client::{client, connect};
pub use crate::error::{Error, Result};
pub use crate::handshake::client::ClientHandshake;
pub use crate::handshake::server::ServerHandshake;
pub use crate::handshake::HandshakeError;
pub use crate::protocol::{Message, WebSocket};
pub use crate::server::{accept, accept_hdr};
