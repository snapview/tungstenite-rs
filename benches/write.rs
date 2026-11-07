//! Benchmarks for write performance.
use criterion::Criterion;
use std::{
    hint, io,
    time::{Duration, Instant},
};
use tungstenite::{protocol::Role, Message, WebSocket};

const MOCK_WRITE_LEN: usize = 8 * 1024 * 1024;

/// `Write` impl that simulates slowish writes and slow flushes.
///
/// Each `write` can buffer up to 8 MiB before flushing but takes an additional **~80ns**
/// to simulate stuff going on in the underlying stream.
/// Each `flush` takes **~8µs** to simulate flush io.
struct MockWrite(Vec<u8>);

impl io::Read for MockWrite {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::new(io::ErrorKind::WouldBlock, "reads not supported"))
    }
}
impl io::Write for MockWrite {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.0.len() + buf.len() > MOCK_WRITE_LEN {
            self.flush()?;
        }
        // simulate io
        spin(Duration::from_nanos(80));
        self.0.extend(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if !self.0.is_empty() {
            // simulate io
            spin(Duration::from_micros(8));
            self.0.clear();
        }
        Ok(())
    }
}

/// `Write` impl that accepts at most 16 KiB per call, like a socket with a small send buffer.
struct ChunkedWrite;

impl io::Read for ChunkedWrite {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::new(io::ErrorKind::WouldBlock, "reads not supported"))
    }
}
impl io::Write for ChunkedWrite {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        Ok(buf.len().min(16 * 1024))
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn spin(duration: Duration) {
    let a = Instant::now();
    while a.elapsed() < duration {
        hint::spin_loop();
    }
}

fn benchmark(c: &mut Criterion) {
    fn write_100k_then_flush(role: Role, b: &mut criterion::Bencher<'_>) {
        let mut ws =
            WebSocket::from_raw_socket(MockWrite(Vec::with_capacity(MOCK_WRITE_LEN)), role, None);

        b.iter(|| {
            for i in 0_u64..100_000 {
                let msg = match i {
                    _ if i % 3 == 0 => Message::binary(i.to_le_bytes().to_vec()),
                    _ => Message::text(format!("{{\"id\":{i}}}")),
                };
                ws.write(msg).unwrap();
            }
            ws.flush().unwrap();
        });
    }

    c.bench_function("write 100k small messages then flush (server)", |b| {
        write_100k_then_flush(Role::Server, b);
    });

    c.bench_function("write+mask 100k small messages then flush (client)", |b| {
        write_100k_then_flush(Role::Client, b);
    });

    c.bench_function("send 1 MiB message in 16 KiB writes (server)", |b| {
        let mut ws = WebSocket::from_raw_socket(ChunkedWrite, Role::Server, None);
        let msg = Message::binary(vec![7; 1024 * 1024]);
        b.iter(|| ws.send(msg.clone()).unwrap());
    });
}

criterion::criterion_group!(write_benches, benchmark);
criterion::criterion_main!(write_benches);
