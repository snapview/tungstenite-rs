//! Benchmarks for read performance.
use criterion::{BatchSize, Criterion};
use std::{
    io::{self, Read, Write},
    sync::{Arc, Mutex},
};
use tungstenite::{protocol::Role, Error, Message, WebSocket};

/// Mock stream with no artificial delays.
#[derive(Default, Clone)]
struct MockIo(Arc<Mutex<Vec<u8>>>);

impl Read for MockIo {
    fn read(&mut self, to: &mut [u8]) -> io::Result<usize> {
        let mut data = self.0.lock().unwrap();
        if data.is_empty() {
            return Err(io::Error::new(io::ErrorKind::WouldBlock, "not ready"));
        }
        let len = data.len().min(to.len());
        to[..len].copy_from_slice(data.drain(..len).as_slice());
        Ok(len)
    }
}

impl Write for MockIo {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Mock stream that returns one small text frame, then `WouldBlock`, and so on.
/// Like a quiet connection that wakes the reader for every message.
#[derive(Default)]
struct OneFrameThenWouldBlock {
    blocked: bool,
}

impl Read for OneFrameThenWouldBlock {
    fn read(&mut self, to: &mut [u8]) -> io::Result<usize> {
        const TEXT_FRAME: &[u8] = b"\x81\x05hello";
        self.blocked = !self.blocked;
        if !self.blocked {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        to[..TEXT_FRAME.len()].copy_from_slice(TEXT_FRAME);
        Ok(TEXT_FRAME.len())
    }
}

impl Write for OneFrameThenWouldBlock {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn benchmark(c: &mut Criterion) {
    /// Benchmark reading 100k mix of binary & text messages.
    fn read_100k(role: Role, b: &mut criterion::Bencher<'_>) {
        let io = MockIo::default();
        let mut writer = WebSocket::from_raw_socket(
            io.clone(),
            match role {
                Role::Client => Role::Server,
                Role::Server => Role::Client,
            },
            None,
        );
        let mut ws = WebSocket::from_raw_socket(io, role, None);

        b.iter_batched(
            || {
                let mut sum = 0;
                for i in 0_u64..100_000 {
                    writer
                        .send(match i {
                            _ if i % 3 == 0 => Message::binary(i.to_le_bytes().to_vec()),
                            _ => Message::text(format!("{{\"id\":{i}}}")),
                        })
                        .unwrap();
                    sum += i;
                }
                sum
            },
            |expected_sum| {
                let mut sum = 0;
                while sum != expected_sum {
                    match ws.read().unwrap() {
                        Message::Binary(v) => {
                            let a: &[u8; 8] = v.as_ref().try_into().unwrap();
                            sum += u64::from_le_bytes(*a);
                        }
                        Message::Text(msg) => {
                            let i: u64 = msg.as_str()[6..msg.len() - 1].parse().unwrap();
                            sum += i;
                        }
                        m => panic!("Unexpected {m}"),
                    }
                }
            },
            BatchSize::SmallInput,
        );
    }

    c.bench_function("read+unmask 100k small messages (server)", |b| {
        read_100k(Role::Server, b);
    });

    c.bench_function("read 100k small messages (client)", |b| {
        read_100k(Role::Client, b);
    });

    c.bench_function("read 1 small message then WouldBlock (client)", |b| {
        let stream = OneFrameThenWouldBlock::default();
        let mut ws = WebSocket::from_raw_socket(stream, Role::Client, None);
        b.iter(|| {
            assert!(matches!(ws.read(), Ok(Message::Text(_))));
            match ws.read() {
                Err(Error::Io(e)) if e.kind() == io::ErrorKind::WouldBlock => {}
                other => panic!("Unexpected {other:?}"),
            }
        });
    });
}

criterion::criterion_group!(read_benches, benchmark);
criterion::criterion_main!(read_benches);
