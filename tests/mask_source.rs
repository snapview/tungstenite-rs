//! A client context draws its frame masks from the installed `MaskSource`.

use std::io::{self, Read, Write};
use tungstenite::{
    protocol::{MaskSource, Role},
    Message, WebSocket,
};

/// A stream that records what is written and never has anything to read.
#[derive(Default)]
struct Wire(Vec<u8>);

impl Read for Wire {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        Err(io::ErrorKind::WouldBlock.into())
    }
}

impl Write for Wire {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Counts upward from a start byte, so each draw is distinct and known.
struct Counting(u8);

impl MaskSource for Counting {
    fn fill(&mut self, buf: &mut [u8]) -> io::Result<()> {
        for b in buf {
            *b = self.0;
            self.0 = self.0.wrapping_add(1);
        }
        Ok(())
    }
}

#[test]
fn client_frames_carry_the_installed_source_masks() {
    let mut ws = WebSocket::from_raw_socket(Wire::default(), Role::Client, None);
    ws.set_mask_source(Counting(1));
    ws.send(Message::text("hi")).unwrap();
    ws.send(Message::text("yo")).unwrap();
    let wire = &ws.get_ref().0;
    // FIN|text, MASK|len 2, the 4-byte mask 1,2,3,4, then the payload xor the mask.
    assert_eq!(wire[..8], [0x81, 0x82, 1, 2, 3, 4, b'h' ^ 1, b'i' ^ 2]);
    // The next frame draws the next four bytes: a fresh mask per frame.
    assert_eq!(wire[8..], [0x81, 0x82, 5, 6, 7, 8, b'y' ^ 5, b'o' ^ 6]);
}

#[test]
fn a_server_context_never_draws_a_mask() {
    let mut ws = WebSocket::from_raw_socket(Wire::default(), Role::Server, None);
    ws.set_mask_source(Counting(1));
    ws.send(Message::text("hi")).unwrap();
    assert_eq!(ws.get_ref().0, [0x81, 0x02, b'h', b'i']);
}

#[cfg(not(feature = "rand"))]
#[test]
fn a_client_with_no_source_and_no_rand_fails_closed() {
    use tungstenite::{error::ProtocolError, Error};
    let mut ws = WebSocket::from_raw_socket(Wire::default(), Role::Client, None);
    match ws.send(Message::text("hi")) {
        Err(Error::Protocol(ProtocolError::MissingMaskSource)) => {}
        other => panic!("want MissingMaskSource, got {other:?}"),
    }
    assert!(ws.get_ref().0.is_empty());
}

#[cfg(feature = "rand")]
#[test]
fn with_rand_and_no_source_the_crate_still_masks() {
    let mut ws = WebSocket::from_raw_socket(Wire::default(), Role::Client, None);
    ws.send(Message::text("hi")).unwrap();
    assert_eq!(ws.get_ref().0[1], 0x82);
}

/// A source whose draw fails.
struct Failing;

impl MaskSource for Failing {
    fn fill(&mut self, _: &mut [u8]) -> io::Result<()> {
        Err(io::Error::other("no entropy"))
    }
}

#[test]
fn a_failed_draw_fails_the_send_and_writes_nothing() {
    let mut ws = WebSocket::from_raw_socket(Wire::default(), Role::Client, None);
    ws.set_mask_source(Failing);
    match ws.send(Message::text("hi")) {
        Err(tungstenite::Error::Io(_)) => {}
        other => panic!("want Error::Io, got {other:?}"),
    }
    assert!(ws.get_ref().0.is_empty());
}
