use std::io::{self, Read, Write};
use tungstenite::{protocol::WebSocketConfig, Message, WebSocket};

/// `Write` impl that records call stats and drops the data.
#[derive(Debug, Default)]
struct MockWrite {
    written_bytes: usize,
    write_count: usize,
    flush_count: usize,
}

impl Read for MockWrite {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::new(io::ErrorKind::WouldBlock, "reads not supported"))
    }
}
impl Write for MockWrite {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.written_bytes += buf.len();
        self.write_count += 1;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.flush_count += 1;
        Ok(())
    }
}

/// Test for write buffering and flushing behaviour.
#[test]
fn write_flush_behaviour() {
    const SEND_ME_LEN: usize = 10;
    const BATCH_ME_LEN: usize = 11;
    const WRITE_BUFFER_SIZE: usize = 600;

    let mut ws = WebSocket::from_raw_socket(
        MockWrite::default(),
        tungstenite::protocol::Role::Server,
        Some(WebSocketConfig::default().write_buffer_size(WRITE_BUFFER_SIZE)),
    );

    assert_eq!(ws.get_ref().written_bytes, 0);
    assert_eq!(ws.get_ref().write_count, 0);
    assert_eq!(ws.get_ref().flush_count, 0);

    // `send` writes & flushes immediately
    ws.send(Message::Text("Send me!".into())).unwrap();
    assert_eq!(ws.get_ref().written_bytes, SEND_ME_LEN);
    assert_eq!(ws.get_ref().write_count, 1);
    assert_eq!(ws.get_ref().flush_count, 1);

    // send a batch of messages
    for msg in (0..100).map(|_| Message::Text("Batch me!".into())) {
        ws.write(msg).unwrap();
    }
    // after 55 writes the out_buffer will exceed write_buffer_size=600
    // and so do a single underlying write (not flushing).
    assert_eq!(ws.get_ref().written_bytes, 55 * BATCH_ME_LEN + SEND_ME_LEN);
    assert_eq!(ws.get_ref().write_count, 2);
    assert_eq!(ws.get_ref().flush_count, 1);

    // flushing will perform a single write for the remaining out_buffer & flush.
    ws.flush().unwrap();
    assert_eq!(ws.get_ref().written_bytes, 100 * BATCH_ME_LEN + SEND_ME_LEN);
    assert_eq!(ws.get_ref().write_count, 3);
    assert_eq!(ws.get_ref().flush_count, 2);
}
