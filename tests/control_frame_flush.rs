//! Verifies that close and pong frames reach the peer when a flush blocks or the write
//! buffer is full.

use std::io::{self, Cursor, Read, Write};
use tungstenite::{
    protocol::{Role, WebSocketConfig},
    Error, Message, WebSocket,
};

/// Keeps written bytes until `flush`, like `BufWriter` or a TLS stream.
/// Reads `incoming`, then `WouldBlock`, like a peer that waits for us.
struct BufferUntilFlush {
    incoming: Cursor<Vec<u8>>,
    buffered: Vec<u8>,
    /// Bytes that were flushed, i.e. sent to the peer.
    wire: Vec<u8>,
    /// The first `flush` returns `WouldBlock`.
    block_first_flush: bool,
}

impl BufferUntilFlush {
    fn new(incoming: &[u8], block_first_flush: bool) -> Self {
        Self {
            incoming: Cursor::new(incoming.to_vec()),
            buffered: Vec::new(),
            wire: Vec::new(),
            block_first_flush,
        }
    }
}

impl Read for BufferUntilFlush {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self.incoming.read(buf)? {
            0 => Err(io::ErrorKind::WouldBlock.into()),
            n => Ok(n),
        }
    }
}

impl Write for BufferUntilFlush {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.buffered.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        if std::mem::take(&mut self.block_first_flush) {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        self.wire.append(&mut self.buffered);
        Ok(())
    }
}

/// An unmasked close frame without a payload, as a server sends it.
const SERVER_CLOSE: [u8; 2] = [0x88, 0x00];
/// A masked close frame without a payload: header, then the 4-byte mask.
const CLIENT_CLOSE_LEN: usize = 6;
/// A masked ping frame with the payload `[1, 2]` and an all-zero mask.
const CLIENT_PING: [u8; 8] = [0x89, 0x82, 0, 0, 0, 0, 1, 2];
/// The pong reply of the server to `CLIENT_PING`.
const SERVER_PONG: [u8; 4] = [0x8a, 0x02, 1, 2];

fn assert_would_block(result: tungstenite::Result<impl std::fmt::Debug>) {
    match result {
        Err(Error::Io(err)) if err.kind() == io::ErrorKind::WouldBlock => {}
        other => panic!("expected WouldBlock, got {other:?}"),
    }
}

#[test]
fn client_read_retries_close_after_close_flush_would_block() {
    let mut ws = WebSocket::from_raw_socket(BufferUntilFlush::new(&[], true), Role::Client, None);

    assert_would_block(ws.close(None));
    assert!(ws.get_ref().wire.is_empty());

    // The server has not replied yet.
    assert_would_block(ws.read());
    assert_eq!(ws.get_ref().wire.len(), CLIENT_CLOSE_LEN);
    assert_eq!(ws.get_ref().wire[0], 0x88);
}

#[test]
fn server_read_retries_close_after_close_flush_would_block() {
    let mut ws = WebSocket::from_raw_socket(BufferUntilFlush::new(&[], true), Role::Server, None);

    assert_would_block(ws.close(None));
    assert!(ws.get_ref().wire.is_empty());

    assert_would_block(ws.read());
    assert_eq!(ws.get_ref().wire, SERVER_CLOSE);
}

#[test]
fn client_read_retries_close_reply_after_flush_would_block() {
    let mut ws =
        WebSocket::from_raw_socket(BufferUntilFlush::new(&SERVER_CLOSE, true), Role::Client, None);
    assert_eq!(ws.read().unwrap(), Message::Close(None));

    assert_would_block(ws.flush());
    assert!(ws.get_ref().wire.is_empty());

    // The server waits for our reply before it closes the connection.
    assert_would_block(ws.read());
    assert_eq!(ws.get_ref().wire.len(), CLIENT_CLOSE_LEN);
    assert_eq!(ws.get_ref().wire[0], 0x88);
}

#[test]
fn flush_sends_pong_that_did_not_fit_next_to_buffered_binary_frame() {
    // A 16-byte binary frame fills the buffer, the 4-byte pong does not fit next to it.
    let config = WebSocketConfig::default().write_buffer_size(16).max_write_buffer_size(17);
    let mut ws = WebSocket::from_raw_socket(
        BufferUntilFlush::new(&CLIENT_PING, false),
        Role::Server,
        Some(config),
    );
    ws.write(Message::binary(vec![7; 14])).unwrap();
    assert_eq!(ws.read().unwrap(), Message::Ping(vec![1, 2].into()));

    ws.flush().unwrap();
    assert_eq!(ws.get_ref().wire.len(), 16 + SERVER_PONG.len());
    assert_eq!(ws.get_ref().wire[16..], SERVER_PONG);
}

#[test]
fn flush_sends_pong_larger_than_max_write_buffer_size() {
    // The smallest limit the config allows.
    let config = WebSocketConfig::default().write_buffer_size(0).max_write_buffer_size(1);
    let mut ws = WebSocket::from_raw_socket(
        BufferUntilFlush::new(&CLIENT_PING, false),
        Role::Server,
        Some(config),
    );
    assert_eq!(ws.read().unwrap(), Message::Ping(vec![1, 2].into()));

    ws.flush().unwrap();
    assert_eq!(ws.get_ref().wire, SERVER_PONG);
}

/// Writes at most `budget` bytes, then returns `WouldBlock`, like a slow peer.
/// Reads `incoming`, then `WouldBlock`.
struct Throttled {
    incoming: Cursor<Vec<u8>>,
    wire: Vec<u8>,
    budget: usize,
    /// Writes return `ConnectionReset`.
    reset: bool,
}

impl Throttled {
    fn new(incoming: &[u8]) -> Self {
        Self { incoming: Cursor::new(incoming.to_vec()), wire: Vec::new(), budget: 0, reset: false }
    }
}

impl Read for Throttled {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self.incoming.read(buf)? {
            0 => Err(io::ErrorKind::WouldBlock.into()),
            n => Ok(n),
        }
    }
}

impl Write for Throttled {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.reset {
            return Err(io::ErrorKind::ConnectionReset.into());
        }
        if self.budget == 0 {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        let n = buf.len().min(self.budget);
        self.wire.extend_from_slice(&buf[..n]);
        self.budget -= n;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn pong_is_sent_while_new_data_keeps_the_write_buffer_busy() {
    let config = WebSocketConfig::default().write_buffer_size(1024).max_write_buffer_size(4096);
    let mut ws =
        WebSocket::from_raw_socket(Throttled::new(&CLIENT_PING), Role::Server, Some(config));
    // A 516-byte binary frame waits in the buffer when the ping arrives.
    ws.write(Message::binary(vec![7; 512])).unwrap();
    assert_eq!(ws.read().unwrap(), Message::Ping(vec![1, 2].into()));

    // Each round the peer takes 256 bytes and we queue a new 256-byte frame.
    for _ in 0..8 {
        ws.get_mut().budget = 256;
        let _ = ws.write(Message::binary(vec![7; 252]));
        let _ = ws.flush();
    }

    let wire = &ws.get_ref().wire;
    assert_eq!(wire.len(), 8 * 256);
    assert!(wire.windows(SERVER_PONG.len()).any(|w| w == SERVER_PONG), "pong not sent");
}

#[test]
fn server_reports_connection_closed_when_peer_resets_after_its_close() {
    // Every frame is written at once, the first one blocks and stays in the buffer.
    let config = WebSocketConfig::default().write_buffer_size(0);
    let mut ws = WebSocket::from_raw_socket(
        Throttled::new(&[0x88, 0x80, 0, 0, 0, 0]),
        Role::Server,
        Some(config),
    );
    assert_would_block(ws.write(Message::binary(vec![7; 14])));
    assert_eq!(ws.read().unwrap(), Message::Close(None));

    ws.get_mut().reset = true;
    match ws.read() {
        Err(Error::ConnectionClosed) => {}
        other => panic!("expected ConnectionClosed, got {other:?}"),
    }
}
