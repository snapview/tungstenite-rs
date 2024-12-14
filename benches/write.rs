//! Benchmarks for write performance.
use bytes::{BufMut, BytesMut};
use criterion::Criterion;
use std::{
    fmt::Write as _,
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

        let mut buf = BytesMut::with_capacity(128 * 1024);

        b.iter(|| {
            for i in 0_u64..100_000 {
                let msg = match i {
                    _ if i % 3 == 0 => {
                        buf.put_slice(&i.to_le_bytes());
                        Message::binary(buf.split())
                    }
                    _ => {
                        buf.write_fmt(format_args!("{{\"id\":{i}}}")).unwrap();
                        Message::Text(buf.split().try_into().unwrap())
                    }
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
}

criterion::criterion_group!(write_benches, benchmark);
criterion::criterion_main!(write_benches);
