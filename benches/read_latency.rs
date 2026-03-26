//! Benchmark the per-message processing cost of `WebSocket::read()` for text
//! messages, isolated from network I/O. Compares different `read_buffer_size`
//! values to quantify the impact of the zero-fill in `FrameCodec::read_in`.
//!
//! Two benchmark groups:
//! - `read_text_500b_throughput`: Cursor delivers all frames at once (measures
//!   throughput when many messages are buffered).
//! - `read_text_500b_latency`: Stream delivers one frame per read() call
//!   (simulates a real socket where each message arrives individually).

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use std::{
    hint::black_box,
    io::{self, Cursor, Read, Write},
};
use tungstenite::{
    protocol::{Role, WebSocketConfig},
    WebSocket,
};

/// Stream that delivers all data as fast as possible (like a Cursor).
/// Writes are discarded.
struct BulkStream(Cursor<Vec<u8>>);

impl Read for BulkStream {
    #[inline]
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf)
    }
}

impl Write for BulkStream {
    #[inline]
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        Ok(buf.len())
    }
    #[inline]
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Stream that delivers at most one WebSocket frame per read() call.
/// This simulates a real socket where messages arrive one at a time,
/// forcing `read_in` to be called for every message — exposing the
/// true per-message cost of the zero-fill.
struct SingleFrameStream {
    data: Vec<u8>,
    pos: usize,
    frame_size: usize,
}

impl SingleFrameStream {
    fn new(data: Vec<u8>, frame_size: usize) -> Self {
        Self { data, pos: 0, frame_size }
    }
}

impl Read for SingleFrameStream {
    #[inline]
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let remaining = self.data.len() - self.pos;
        if remaining == 0 {
            return Ok(0);
        }
        // Limit to one frame per read, simulating per-message TCP delivery.
        let n = buf.len().min(remaining).min(self.frame_size);
        buf[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

impl Write for SingleFrameStream {
    #[inline]
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        Ok(buf.len())
    }
    #[inline]
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Construct an unmasked WebSocket text frame (server → client).
///
/// Header sizes: ≤125 bytes → 2B, 126–65535 → 4B, >65535 → 10B.
fn make_unmasked_text_frame(payload: &[u8]) -> Vec<u8> {
    let len = payload.len();
    let header_size = if len <= 125 {
        2
    } else if len <= 65535 {
        4
    } else {
        10
    };
    let mut frame = Vec::with_capacity(header_size + len);

    // FIN=1, RSV=0, opcode=0x1 (text)
    frame.push(0x81);

    if len <= 125 {
        frame.push(len as u8);
    } else if len <= 65535 {
        // MASK=0, length marker=126 → next 2 bytes are big-endian u16 length
        frame.push(126);
        frame.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        // MASK=0, length marker=127 → next 8 bytes are big-endian u64 length
        frame.push(127);
        frame.extend_from_slice(&(len as u64).to_be_bytes());
    }

    frame.extend_from_slice(payload);
    frame
}

fn make_payload() -> (String, Vec<u8>) {
    let base = r#"{"id":12345,"type":"trade","symbol":"BTCUSD","price":"67234.50","quantity":"0.0423","timestamp":1711382400000,"side":"buy","exchange":"exchange","sequence":98765432}"#;
    let padding_needed = 500 - (base.len() - 1) - r#","pad":""}"#.len();
    let payload = format!(
        "{},\"pad\":\"{}\"}}",
        &base[..base.len() - 1],
        "x".repeat(padding_needed)
    );
    assert_eq!(payload.len(), 500, "payload must be exactly 500 bytes");
    let frame = make_unmasked_text_frame(payload.as_bytes());
    (payload, frame)
}

/// Throughput benchmark: Cursor delivers all frames as a blob.
/// Large buffers amortize `read_in` across many frames per read() call.
fn bench_throughput(c: &mut Criterion) {
    let (_payload, frame) = make_payload();
    let msg_count: usize = 1000;
    let all_frames: Vec<u8> = frame.repeat(msg_count);

    let mut group = c.benchmark_group("read_text_500b_throughput");
    group.throughput(Throughput::Elements(msg_count as u64));

    for buf_size in [1024, 2048, 4096, 8192, 16384, 32768, 65536, 128 * 1024] {
        group.bench_with_input(
            BenchmarkId::from_parameter(buf_size),
            &buf_size,
            |b, &buf_size| {
                let config = WebSocketConfig::default().read_buffer_size(buf_size);
                b.iter_batched(
                    || {
                        let stream = BulkStream(Cursor::new(all_frames.clone()));
                        WebSocket::from_raw_socket(stream, Role::Client, Some(config))
                    },
                    |mut ws| {
                        for _ in 0..msg_count {
                            black_box(ws.read().unwrap());
                        }
                    },
                    criterion::BatchSize::LargeInput,
                );
            },
        );
    }
    group.finish();
}

/// Latency benchmark: stream delivers exactly one frame per read() call.
/// This forces `read_in` to be called for every message, exposing the
/// true per-message cost of zeroing the buffer.
fn bench_latency(c: &mut Criterion) {
    let (_payload, frame) = make_payload();
    let frame_size = frame.len();
    let msg_count: usize = 1000;
    let all_frames: Vec<u8> = frame.repeat(msg_count);

    let mut group = c.benchmark_group("read_text_500b_latency");
    group.throughput(Throughput::Elements(msg_count as u64));

    for buf_size in [1024, 2048, 4096, 8192, 16384, 32768, 65536, 128 * 1024] {
        group.bench_with_input(
            BenchmarkId::from_parameter(buf_size),
            &buf_size,
            |b, &buf_size| {
                let config = WebSocketConfig::default().read_buffer_size(buf_size);
                b.iter_batched(
                    || {
                        let stream =
                            SingleFrameStream::new(all_frames.clone(), frame_size);
                        WebSocket::from_raw_socket(stream, Role::Client, Some(config))
                    },
                    |mut ws| {
                        for _ in 0..msg_count {
                            black_box(ws.read().unwrap());
                        }
                    },
                    criterion::BatchSize::LargeInput,
                );
            },
        );
    }
    group.finish();
}

/// Correctness + performance for messages larger than the default read buffer.
/// A 1 MiB payload with a 128 KiB buffer requires ~8 `read_in` calls,
/// exercising buffer reallocation and multi-pass reads.
fn bench_large_messages(c: &mut Criterion) {
    let mut group = c.benchmark_group("read_text_large");

    for payload_size in [256 * 1024, 1024 * 1024] {
        let payload: String = (0..payload_size)
            .map(|i| (b'A' + (i % 26) as u8) as char)
            .collect();
        let frame = make_unmasked_text_frame(payload.as_bytes());
        let frame_size = frame.len();
        let msg_count: usize = 50;
        let all_frames: Vec<u8> = frame.repeat(msg_count);

        group.throughput(Throughput::Elements(msg_count as u64));

        // Use SingleFrameStream so each message triggers its own read_in
        // calls — same as real-world per-message arrival.
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{}KiB", payload_size / 1024)),
            &payload_size,
            |b, _| {
                let config = WebSocketConfig::default()
                    .max_message_size(Some(payload_size + 1))
                    .max_frame_size(Some(payload_size + 1));
                b.iter_batched(
                    || {
                        let stream =
                            SingleFrameStream::new(all_frames.clone(), frame_size);
                        WebSocket::from_raw_socket(stream, Role::Client, Some(config))
                    },
                    |mut ws| {
                        for _ in 0..msg_count {
                            let msg = ws.read().unwrap();
                            // Verify first and last bytes to catch corruption.
                            match &msg {
                                tungstenite::Message::Text(t) => {
                                    debug_assert_eq!(t.as_bytes()[0], b'A');
                                    debug_assert_eq!(
                                        t.as_bytes()[payload_size - 1],
                                        b'A' + ((payload_size - 1) % 26) as u8
                                    );
                                }
                                other => panic!("Expected Text, got {other:?}"),
                            }
                            black_box(msg);
                        }
                    },
                    criterion::BatchSize::LargeInput,
                );
            },
        );
    }
    group.finish();
}

criterion_group!(benches, bench_throughput, bench_latency, bench_large_messages);
criterion_main!(benches);
