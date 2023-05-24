//! Benchmarks for write performance.
use criterion::{BatchSize, Criterion};
use std::{
    hint,
    io::{self, Read, Write},
    time::{Duration, Instant},
};
use tungstenite::{Message, WebSocket};

const MOCK_WRITE_LEN: usize = 8 * 1024 * 1024;

/// `Write` impl that simulates fast writes and slow flushes.
///
/// Buffers up to 8 MiB fast on `write`. Each `flush` takes ~100ns.
struct MockSlowFlushWrite(Vec<u8>);

impl Read for MockSlowFlushWrite {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::new(io::ErrorKind::WouldBlock, "reads not supported"))
    }
}
impl Write for MockSlowFlushWrite {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.0.len() + buf.len() > MOCK_WRITE_LEN {
            self.flush()?;
        }
        self.0.extend(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if !self.0.is_empty() {
            // simulate 100ns io
            let a = Instant::now();
            while a.elapsed() < Duration::from_nanos(100) {
                hint::spin_loop();
            }
            self.0.clear();
        }
        Ok(())
    }
}

fn benchmark(c: &mut Criterion) {
    // Writes 100k small json text messages then calls `write_pending`
    c.bench_function("write 100k small texts then flush", |b| {
        let mut ws = WebSocket::from_raw_socket(
            MockSlowFlushWrite(Vec::with_capacity(MOCK_WRITE_LEN)),
            tungstenite::protocol::Role::Server,
            None,
        );

        b.iter_batched(
            || (0..100_000).map(|i| Message::Text(format!("{{\"id\":{i}}}"))),
            |batch| {
                for msg in batch {
                    ws.write_message(msg).unwrap();
                }
                ws.write_pending().unwrap();
            },
            BatchSize::SmallInput,
        )
    });
}

criterion::criterion_group!(write_benches, benchmark);
criterion::criterion_main!(write_benches);
