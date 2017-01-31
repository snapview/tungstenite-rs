//! Lightweight, flexible WebSockets for Rust.
#![deny(
    missing_copy_implementations,
    trivial_casts, trivial_numeric_casts,
    unstable_features,
    unused_must_use,
    unused_mut,
    unused_imports,
    unused_import_braces)]

#[macro_use] extern crate log;
extern crate base64;
extern crate byteorder;
extern crate bytes;
extern crate httparse;
extern crate rand;
extern crate sha1;
extern crate url;
extern crate utf8;
#[cfg(feature="tls")] extern crate native_tls;

pub mod error;
pub mod protocol;
pub mod client;
pub mod server;
pub mod handshake;

mod input_buffer;
mod stream;
