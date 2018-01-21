//! Lightweight, flexible WebSockets for Rust.
#![deny(missing_docs, missing_copy_implementations, missing_debug_implementations, trivial_casts,
        trivial_numeric_casts, unstable_features, unused_must_use, unused_mut, unused_imports,
        unused_import_braces)]

extern crate base64;
extern crate byteorder;
extern crate bytes;
extern crate httparse;
extern crate input_buffer;
#[macro_use]
extern crate log;
#[cfg(feature = "tls")]
extern crate native_tls;
extern crate rand;
extern crate sha1;
extern crate url;
extern crate utf8;

pub mod error;
pub mod protocol;
pub mod client;
pub mod server;
pub mod handshake;
pub mod stream;
pub mod util;

pub use client::{client, connect};
pub use server::{accept, accept_hdr};
pub use error::{Error, Result};
pub use protocol::{Message, WebSocket};
pub use handshake::HandshakeError;
pub use handshake::client::ClientHandshake;
pub use handshake::server::ServerHandshake;
