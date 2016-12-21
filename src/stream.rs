#[cfg(feature="tls")]
use native_tls::TlsStream;

use std::net::TcpStream;
use std::io::{Read, Write, Result as IoResult};

/// Stream, either plain TCP or TLS.
pub enum Stream {
    Plain(TcpStream),
    #[cfg(feature="tls")]
    Tls(TlsStream<TcpStream>),
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> IoResult<usize> {
        match *self {
            Stream::Plain(ref mut s) => s.read(buf),
            #[cfg(feature="tls")]
            Stream::Tls(ref mut s) => s.read(buf),
        }
    }
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> IoResult<usize> {
        match *self {
            Stream::Plain(ref mut s) => s.write(buf),
            #[cfg(feature="tls")]
            Stream::Tls(ref mut s) => s.write(buf),
        }
    }
    fn flush(&mut self) -> IoResult<()> {
        match *self {
            Stream::Plain(ref mut s) => s.flush(),
            #[cfg(feature="tls")]
            Stream::Tls(ref mut s) => s.flush(),
        }
    }
}
