use std::{
    io::{self, Cursor, Read, Write},
    mem,
};
use tungstenite::{
    protocol::frame::{
        coding::{Control, OpCode},
        Frame, FrameHeader,
    },
    Message, WebSocket,
};

const NUMBER_OF_FLUSHES_TO_GET_IT_TO_WORK: usize = 3;

/// `Read`/`Write` mock.
/// * Reads a single ping, then returns `WouldBlock` forever after.
/// * Writes work fine.
/// * Flush `WouldBlock` twice then works on the 3rd attempt.
#[derive(Debug, Default)]
struct MockWrite {
    /// Data written, but not flushed.
    written_data: Vec<u8>,
    /// The latest successfully flushed data.
    flushed_data: Vec<u8>,
    write_calls: usize,
    flush_calls: usize,
    read_calls: usize,
}

impl Read for MockWrite {
    fn read(&mut self, mut buf: &mut [u8]) -> io::Result<usize> {
        self.read_calls += 1;
        if self.read_calls == 1 {
            let ping = Frame::ping(vec![]);
            let len = ping.len();
            ping.format(&mut buf).expect("format failed");
            Ok(len)
        } else {
            Err(io::Error::new(io::ErrorKind::WouldBlock, "nothing else to read"))
        }
    }
}
impl Write for MockWrite {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.write_calls += 1;
        self.written_data.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.flush_calls += 1;
        if self.flush_calls % NUMBER_OF_FLUSHES_TO_GET_IT_TO_WORK == 0 {
            mem::swap(&mut self.written_data, &mut self.flushed_data);
            self.written_data.clear();
            eprintln!("flush success");
            Ok(())
        } else {
            eprintln!("flush would block");
            Err(io::Error::new(io::ErrorKind::WouldBlock, "try again"))
        }
    }
}

/// Test for auto pong write & flushing behaviour.
///
/// In read-only/read-predominant usage auto pong responses should be written and flushed
/// even if WouldBlock errors are encountered.
#[test]
fn read_usage_auto_pong_flush() {
    let mut ws =
        WebSocket::from_raw_socket(MockWrite::default(), tungstenite::protocol::Role::Client, None);

    // Receiving a ping should auto scheduled a pong on next read or write (but not written yet).
    let msg = ws.read().unwrap();
    assert!(matches!(msg, Message::Ping(_)), "Unexpected msg {:?}", msg);
    assert_eq!(ws.get_ref().read_calls, 1);
    assert!(ws.get_ref().written_data.is_empty(), "Unexpected {:?}", ws.get_ref());
    assert!(ws.get_ref().flushed_data.is_empty(), "Unexpected {:?}", ws.get_ref());

    // Next read fails as there is nothing else to read.
    // This read call should have tried to write & flush a pong response, with the flush WouldBlock-ing
    let next = ws.read().unwrap_err();
    assert!(
        matches!(next, tungstenite::Error::Io(ref err) if err.kind() == io::ErrorKind::WouldBlock),
        "Unexpected read err {:?}",
        next
    );
    assert_eq!(ws.get_ref().read_calls, 2);
    assert!(!ws.get_ref().written_data.is_empty(), "Should have written a pong frame");
    assert_eq!(ws.get_ref().write_calls, 1);

    let pong_header =
        FrameHeader::parse(&mut Cursor::new(&ws.get_ref().written_data)).unwrap().unwrap().0;
    assert_eq!(pong_header.opcode, OpCode::Control(Control::Pong));
    let written_data = ws.get_ref().written_data.clone();

    assert_eq!(ws.get_ref().flush_calls, 1);
    assert!(ws.get_ref().flushed_data.is_empty(), "Unexpected {:?}", ws.get_ref());

    // Next read fails as before.
    // This read call should try to flush the pong again, which again WouldBlock
    let next = ws.read().unwrap_err();
    assert!(
        matches!(next, tungstenite::Error::Io(ref err) if err.kind() == io::ErrorKind::WouldBlock),
        "Unexpected read err {:?}",
        next
    );
    assert_eq!(ws.get_ref().read_calls, 3);
    assert_eq!(ws.get_ref().write_calls, 1);
    assert_eq!(ws.get_ref().flush_calls, 2);
    assert!(ws.get_ref().flushed_data.is_empty(), "Unexpected {:?}", ws.get_ref());

    // Next read fails as before.
    // This read call should try to flush the pong again, 3rd flush attempt is the charm
    let next = ws.read().unwrap_err();
    assert!(
        matches!(next, tungstenite::Error::Io(ref err) if err.kind() == io::ErrorKind::WouldBlock),
        "Unexpected read err {:?}",
        next
    );
    assert_eq!(ws.get_ref().read_calls, 4);
    assert_eq!(ws.get_ref().write_calls, 1);
    assert_eq!(ws.get_ref().flush_calls, 3);
    assert!(ws.get_ref().flushed_data == written_data, "Unexpected {:?}", ws.get_ref());

    // On following read calls no additional writes or flushes are necessary
    ws.read().unwrap_err();
    assert_eq!(ws.get_ref().read_calls, 5);
    assert_eq!(ws.get_ref().write_calls, 1);
    assert_eq!(ws.get_ref().flush_calls, 3);
}
