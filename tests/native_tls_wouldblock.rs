//! Regression test for https://github.com/snapview/tungstenite-rs/issues/450
//!
//! A non-blocking stream that reports `WouldBlock` mid TLS handshake must not
//! crash the process. The native-tls backend used to `panic!("Bug: TLS
//! handshake not blocked")`; it must instead surface the interruption as
//! `HandshakeError::Interrupted`, symmetric to the rustls backend, so the
//! caller can retry once the transport is ready.
#![cfg(feature = "native-tls")]

use std::io::{self, Read, Write};

use tungstenite::{client_tls, stream::MaybeTlsStream, HandshakeError};

/// A stream that always reports `WouldBlock` on read and silently accepts
/// writes, simulating a non-blocking socket that is still mid TLS handshake.
struct WouldBlockStream;

impl Read for WouldBlockStream {
    fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::from(io::ErrorKind::WouldBlock))
    }
}

impl Write for WouldBlockStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn native_tls_handshake_wouldblock_is_interrupted_not_panic() {
    // Before the fix this panics with "Bug: TLS handshake not blocked".
    // After the fix the interrupted handshake is reported the same way the
    // rustls backend reports it: `HandshakeError::Interrupted`, which lets the
    // caller retry once the transport is ready again.
    let result = client_tls("wss://example.com/socket", WouldBlockStream);
    match result {
        Err(HandshakeError::Interrupted(_)) => {}
        Err(other) => panic!("expected HandshakeError::Interrupted, got: {other:?}"),
        Ok(_) => panic!("handshake unexpectedly succeeded over a would-block stream"),
    }
}

#[test]
fn native_tls_handshake_wouldblock_inner_stream_is_none_mid_handshake() {
    // While the TLS handshake is still pending, the wrapper exposes no
    // completed `native_tls::TlsStream` yet.
    match client_tls("wss://example.com/socket", WouldBlockStream) {
        Err(HandshakeError::Interrupted(mid)) => match mid.get_ref().get_ref() {
            MaybeTlsStream::NativeTls(native) => {
                assert!(native.get_ref().is_none(), "TlsStream must be None mid-handshake");
            }
            _ => panic!("expected the native-tls stream variant"),
        },
        Err(other) => panic!("expected HandshakeError::Interrupted, got: {other:?}"),
        Ok(_) => panic!("handshake unexpectedly succeeded over a would-block stream"),
    }
}
