//! Control frames sent by the local side must have a payload of 125 bytes or less
//! (RFC 6455 5.5). A close payload is a 2-byte code plus the reason.

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

fn close_frame(reason_len: usize) -> Option<CloseFrame> {
    Some(CloseFrame { code: CloseCode::Normal, reason: "a".repeat(reason_len).into() })
}

fn is_control_frame_too_big<T>(result: tungstenite::Result<T>) -> bool {
    matches!(result, Err(Error::Protocol(ProtocolError::ControlFrameTooBig)))
}

#[test]
fn close_with_124_byte_reason_fails_and_sends_nothing() {
    let mut ws = server();

    assert!(is_control_frame_too_big(ws.close(close_frame(124))));
    assert!(ws.get_ref().0.is_empty());

    // The socket is still open, so the caller can close with a shorter reason.
    ws.close(close_frame(123)).unwrap();
    assert_eq!(ws.get_ref().0[..2], [0x88, 125]);
    assert_eq!(ws.get_ref().0.len(), 2 + 125);
}

#[test]
fn send_close_message_with_124_byte_reason_fails_and_sends_nothing() {
    let mut ws = server();

    assert!(is_control_frame_too_big(ws.send(Message::Close(close_frame(124)))));
    assert!(ws.get_ref().0.is_empty());

    ws.send(Message::Text("still open".into())).unwrap();
    assert_eq!(ws.get_ref().0[..2], [0x81, 10]);
}

#[test]
fn ping_and_pong_with_126_byte_payload_fail_and_send_nothing() {
    let mut ws = server();

    assert!(is_control_frame_too_big(ws.send(Message::Ping(vec![0; 126].into()))));
    assert!(is_control_frame_too_big(ws.send(Message::Pong(vec![0; 126].into()))));
    assert!(ws.get_ref().0.is_empty());

    ws.send(Message::Ping(vec![0; 125].into())).unwrap();
    ws.send(Message::Pong(vec![0; 125].into())).unwrap();
    assert_eq!(ws.get_ref().0.len(), 2 * (2 + 125));
}
