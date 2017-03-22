//! Convenience wrapper for streams to switch between plain TCP and TLS at runtime.
//!
//!  There is no dependency on actual TLS implementations. Everything like
//! `native_tls` or `openssl` will work as long as there is a TLS stream supporting standard
//! `Read + Write` traits.

use std::io::{Read, Write, Result as IoResult};

/// Stream mode, either plain TCP or TLS.
#[derive(Clone, Copy)]
pub enum Mode {
    Plain,
    Tls,
}

/// Stream, either plain TCP or TLS.
pub enum Stream<S, T> {
    Plain(S),
    Tls(T),
}

impl<S: Read, T: Read> Read for Stream<S, T> {
    fn read(&mut self, buf: &mut [u8]) -> IoResult<usize> {
        match *self {
            Stream::Plain(ref mut s) => s.read(buf),
            Stream::Tls(ref mut s) => s.read(buf),
        }
    }
}

impl<S: Write, T: Write> Write for Stream<S, T> {
    fn write(&mut self, buf: &[u8]) -> IoResult<usize> {
        match *self {
            Stream::Plain(ref mut s) => s.write(buf),
            Stream::Tls(ref mut s) => s.write(buf),
        }
    }
    fn flush(&mut self) -> IoResult<()> {
        match *self {
            Stream::Plain(ref mut s) => s.flush(),
            Stream::Tls(ref mut s) => s.flush(),
        }
    }
}
