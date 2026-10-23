//! A close frame must not carry a code that RFC 6455 7.4 forbids on the wire.
//! The rule is the one the read side applies to a received close code.

use std::io::{self, Read, Write};
use tungstenite::{
    error::ProtocolError,
    protocol::{frame::coding::CloseCode, CloseFrame, Role},
    Error, Message, WebSocket,
};

/// Records every written byte. Reads would block.
#[derive(Debug, Default)]
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

fn server() -> WebSocket<Wire> {
    WebSocket::from_raw_socket(Wire::default(), Role::Server, None)
}

fn close_frame(code: CloseCode) -> Option<CloseFrame> {
    Some(CloseFrame { code, reason: "".into() })
}

fn is_invalid_close_sequence<T>(result: tungstenite::Result<T>) -> bool {
    matches!(result, Err(Error::Protocol(ProtocolError::InvalidCloseSequence)))
}

#[test]
fn close_with_1005_1006_or_1015_fails_and_sends_nothing() {
    for code in [CloseCode::Status, CloseCode::Abnormal, CloseCode::Tls] {
        let mut ws = server();

        assert!(is_invalid_close_sequence(ws.close(close_frame(code))), "{code}");
        assert!(ws.get_ref().0.is_empty(), "{code}");

        // The socket is still open, so the caller can close with a valid code.
        ws.close(close_frame(CloseCode::Normal)).unwrap();
        assert_eq!(ws.get_ref().0, [0x88, 2, 0x03, 0xe8]);
    }
}

#[test]
fn close_with_forbidden_number_in_any_variant_fails() {
    let forbidden = [
        CloseCode::Library(0),
        CloseCode::Library(999),
        CloseCode::Library(1004),
        CloseCode::Library(1005),
        CloseCode::Iana(1006),
        CloseCode::Library(1014),
        CloseCode::Library(1015),
        CloseCode::Library(2999),
        CloseCode::Iana(5000),
    ];
    for code in forbidden {
        let mut ws = server();

        assert!(is_invalid_close_sequence(ws.close(close_frame(code))), "{code}");
        assert!(ws.get_ref().0.is_empty(), "{code}");
    }
}

#[test]
fn send_close_message_with_1006_fails_and_sends_nothing() {
    let mut ws = server();

    assert!(is_invalid_close_sequence(ws.send(Message::Close(close_frame(CloseCode::Abnormal)))));
    assert!(ws.get_ref().0.is_empty());

    ws.send(Message::Text("still open".into())).unwrap();
    assert_eq!(ws.get_ref().0[..2], [0x81, 10]);
}

#[test]
fn close_with_allowed_code_sends_it() {
    for code in [1000, 1003, 1007, 1013, 3000, 4999] {
        let mut ws = server();

        ws.close(close_frame(code.into())).unwrap();
        let [hi, lo] = u16::to_be_bytes(code);
        assert_eq!(ws.get_ref().0, [0x88, 2, hi, lo], "{code}");
    }
}
