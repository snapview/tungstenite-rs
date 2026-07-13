//! Convenience wrapper for streams to switch between plain TCP and TLS at runtime.
//!
//!  There is no dependency on actual TLS implementations. Everything like
//! `native_tls` or `openssl` will work as long as there is a TLS stream supporting standard
//! `Read + Write` traits.

#[cfg(feature = "__rustls-tls")]
use std::ops::Deref;
use std::{
    fmt::{self, Debug},
    io::{Read, Result as IoResult, Write},
};

use std::net::TcpStream;

#[cfg(feature = "native-tls")]
use native_tls_crate::{HandshakeError as NativeHandshakeError, MidHandshakeTlsStream, TlsStream};
#[cfg(feature = "__rustls-tls")]
use rustls::StreamOwned;

/// Stream mode, either plain TCP or TLS.
#[derive(Clone, Copy, Debug)]
pub enum Mode {
    /// Plain mode (`ws://` URL).
    Plain,
    /// TLS mode (`wss://` URL).
    Tls,
}

/// Trait to switch TCP_NODELAY.
pub trait NoDelay {
    /// Set the TCP_NODELAY option to the given value.
    fn set_nodelay(&mut self, nodelay: bool) -> IoResult<()>;
}

impl NoDelay for TcpStream {
    fn set_nodelay(&mut self, nodelay: bool) -> IoResult<()> {
        TcpStream::set_nodelay(self, nodelay)
    }
}

#[cfg(feature = "native-tls")]
impl<S: Read + Write + NoDelay> NoDelay for TlsStream<S> {
    fn set_nodelay(&mut self, nodelay: bool) -> IoResult<()> {
        self.get_mut().set_nodelay(nodelay)
    }
}

/// A `native-tls` stream that may still be completing its TLS handshake.
///
/// The `native-tls` `TlsConnector::connect` drives the TLS handshake eagerly,
/// so on a non-blocking transport it can return before the handshake is
/// finished. This wrapper stores that mid-handshake state and transparently
/// resumes it on the next read or write, mirroring the way the `rustls` backend
/// defers its handshake until the first I/O operation. Keeping the two backends
/// symmetric means an interrupted handshake surfaces as
/// [`HandshakeError::Interrupted`](crate::HandshakeError::Interrupted) instead
/// of panicking (see <https://github.com/snapview/tungstenite-rs/issues/450>).
#[cfg(feature = "native-tls")]
pub struct NativeTlsStream<S: Read + Write> {
    state: NativeTlsState<S>,
}

#[cfg(feature = "native-tls")]
#[allow(clippy::large_enum_variant)]
enum NativeTlsState<S: Read + Write> {
    /// The TLS handshake is still in progress; it is resumed on the next I/O.
    Handshaking(MidHandshakeTlsStream<S>),
    /// The TLS handshake has completed; I/O is delegated to the stream.
    Ready(TlsStream<S>),
    /// Transient placeholder used while resuming the handshake, and the terminal
    /// state after a fatal handshake error. Any I/O in this state fails.
    Invalid,
}

#[cfg(feature = "native-tls")]
impl<S: Read + Write> NativeTlsStream<S> {
    /// Wrap an already-completed `native-tls` stream.
    pub(crate) fn ready(stream: TlsStream<S>) -> Self {
        Self { state: NativeTlsState::Ready(stream) }
    }

    /// Wrap a `native-tls` stream whose handshake was interrupted (would block).
    pub(crate) fn handshaking(stream: MidHandshakeTlsStream<S>) -> Self {
        Self { state: NativeTlsState::Handshaking(stream) }
    }

    /// Returns a reference to the underlying `native-tls` stream, or `None` if
    /// the TLS handshake has not finished yet.
    pub fn get_ref(&self) -> Option<&TlsStream<S>> {
        match &self.state {
            NativeTlsState::Ready(s) => Some(s),
            _ => None,
        }
    }

    /// Returns a mutable reference to the underlying `native-tls` stream, or
    /// `None` if the TLS handshake has not finished yet.
    pub fn get_mut(&mut self) -> Option<&mut TlsStream<S>> {
        match &mut self.state {
            NativeTlsState::Ready(s) => Some(s),
            _ => None,
        }
    }

    /// Consumes the wrapper, returning the underlying `native-tls` stream, or
    /// `None` if the TLS handshake has not finished yet.
    pub fn into_inner(self) -> Option<TlsStream<S>> {
        match self.state {
            NativeTlsState::Ready(s) => Some(s),
            _ => None,
        }
    }

    /// Drive the handshake to completion if it is still pending. Returns `Ok`
    /// once the stream is ready for I/O. If the handshake would block, the
    /// mid-handshake state is kept so the operation can be retried later.
    fn resume(&mut self) -> IoResult<()> {
        if let NativeTlsState::Ready(_) = self.state {
            return Ok(());
        }
        match std::mem::replace(&mut self.state, NativeTlsState::Invalid) {
            NativeTlsState::Handshaking(mid) => match mid.handshake() {
                Ok(stream) => {
                    self.state = NativeTlsState::Ready(stream);
                    Ok(())
                }
                Err(NativeHandshakeError::WouldBlock(mid)) => {
                    self.state = NativeTlsState::Handshaking(mid);
                    Err(std::io::ErrorKind::WouldBlock.into())
                }
                Err(NativeHandshakeError::Failure(err)) => Err(std::io::Error::other(err)),
            },
            // `Ready` is handled above, so only `Invalid` reaches this arm.
            _ => Err(std::io::Error::other("native-tls stream used after a failed handshake")),
        }
    }
}

#[cfg(feature = "native-tls")]
impl<S: Read + Write + Debug> Debug for NativeTlsStream<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.state {
            NativeTlsState::Handshaking(_) => {
                f.debug_tuple("NativeTlsStream::Handshaking").finish()
            }
            NativeTlsState::Ready(s) => f.debug_tuple("NativeTlsStream::Ready").field(s).finish(),
            NativeTlsState::Invalid => f.debug_tuple("NativeTlsStream::Invalid").finish(),
        }
    }
}

#[cfg(feature = "native-tls")]
impl<S: Read + Write> Read for NativeTlsStream<S> {
    fn read(&mut self, buf: &mut [u8]) -> IoResult<usize> {
        self.resume()?;
        match &mut self.state {
            NativeTlsState::Ready(s) => s.read(buf),
            // `resume` only returns `Ok` when the stream is `Ready`.
            _ => Err(std::io::ErrorKind::WouldBlock.into()),
        }
    }
}

#[cfg(feature = "native-tls")]
impl<S: Read + Write> Write for NativeTlsStream<S> {
    fn write(&mut self, buf: &[u8]) -> IoResult<usize> {
        self.resume()?;
        match &mut self.state {
            NativeTlsState::Ready(s) => s.write(buf),
            _ => Err(std::io::ErrorKind::WouldBlock.into()),
        }
    }

    fn flush(&mut self) -> IoResult<()> {
        self.resume()?;
        match &mut self.state {
            NativeTlsState::Ready(s) => s.flush(),
            _ => Err(std::io::ErrorKind::WouldBlock.into()),
        }
    }
}

#[cfg(feature = "native-tls")]
impl<S: Read + Write + NoDelay> NoDelay for NativeTlsStream<S> {
    fn set_nodelay(&mut self, nodelay: bool) -> IoResult<()> {
        match &mut self.state {
            NativeTlsState::Handshaking(mid) => mid.get_mut().set_nodelay(nodelay),
            NativeTlsState::Ready(s) => s.get_mut().set_nodelay(nodelay),
            NativeTlsState::Invalid => Ok(()),
        }
    }
}

#[cfg(feature = "__rustls-tls")]
impl<S, SD, T> NoDelay for StreamOwned<S, T>
where
    S: Deref<Target = rustls::ConnectionCommon<SD>>,
    SD: rustls::SideData,
    T: Read + Write + NoDelay,
{
    fn set_nodelay(&mut self, nodelay: bool) -> IoResult<()> {
        self.sock.set_nodelay(nodelay)
    }
}

/// A stream that might be protected with TLS.
#[non_exhaustive]
#[allow(clippy::large_enum_variant)]
pub enum MaybeTlsStream<S: Read + Write> {
    /// Unencrypted socket stream.
    Plain(S),
    #[cfg(feature = "native-tls")]
    /// Encrypted socket stream using `native-tls`.
    NativeTls(NativeTlsStream<S>),
    #[cfg(feature = "__rustls-tls")]
    /// Encrypted socket stream using `rustls`.
    Rustls(rustls::StreamOwned<rustls::ClientConnection, S>),
}

impl<S: Read + Write + Debug> Debug for MaybeTlsStream<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Plain(s) => f.debug_tuple("MaybeTlsStream::Plain").field(s).finish(),
            #[cfg(feature = "native-tls")]
            Self::NativeTls(s) => f.debug_tuple("MaybeTlsStream::NativeTls").field(s).finish(),
            #[cfg(feature = "__rustls-tls")]
            Self::Rustls(s) => {
                struct RustlsStreamDebug<'a, S: Read + Write>(
                    &'a rustls::StreamOwned<rustls::ClientConnection, S>,
                );

                impl<S: Read + Write + Debug> Debug for RustlsStreamDebug<'_, S> {
                    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                        f.debug_struct("StreamOwned")
                            .field("conn", &self.0.conn)
                            .field("sock", &self.0.sock)
                            .finish()
                    }
                }

                f.debug_tuple("MaybeTlsStream::Rustls").field(&RustlsStreamDebug(s)).finish()
            }
        }
    }
}

impl<S: Read + Write> Read for MaybeTlsStream<S> {
    fn read(&mut self, buf: &mut [u8]) -> IoResult<usize> {
        match *self {
            MaybeTlsStream::Plain(ref mut s) => s.read(buf),
            #[cfg(feature = "native-tls")]
            MaybeTlsStream::NativeTls(ref mut s) => s.read(buf),
            #[cfg(feature = "__rustls-tls")]
            MaybeTlsStream::Rustls(ref mut s) => s.read(buf),
        }
    }
}

impl<S: Read + Write> Write for MaybeTlsStream<S> {
    fn write(&mut self, buf: &[u8]) -> IoResult<usize> {
        match *self {
            MaybeTlsStream::Plain(ref mut s) => s.write(buf),
            #[cfg(feature = "native-tls")]
            MaybeTlsStream::NativeTls(ref mut s) => s.write(buf),
            #[cfg(feature = "__rustls-tls")]
            MaybeTlsStream::Rustls(ref mut s) => s.write(buf),
        }
    }

    fn flush(&mut self) -> IoResult<()> {
        match *self {
            MaybeTlsStream::Plain(ref mut s) => s.flush(),
            #[cfg(feature = "native-tls")]
            MaybeTlsStream::NativeTls(ref mut s) => s.flush(),
            #[cfg(feature = "__rustls-tls")]
            MaybeTlsStream::Rustls(ref mut s) => s.flush(),
        }
    }
}

impl<S: Read + Write + NoDelay> NoDelay for MaybeTlsStream<S> {
    fn set_nodelay(&mut self, nodelay: bool) -> IoResult<()> {
        match *self {
            MaybeTlsStream::Plain(ref mut s) => s.set_nodelay(nodelay),
            #[cfg(feature = "native-tls")]
            MaybeTlsStream::NativeTls(ref mut s) => s.set_nodelay(nodelay),
            #[cfg(feature = "__rustls-tls")]
            MaybeTlsStream::Rustls(ref mut s) => s.set_nodelay(nodelay),
        }
    }
}
