use std::io::{Cursor, Read};

use criterion::*;
use input_buffer::InputBuffer;
use tungstenite::buffer::ReadBuffer;

const CHUNK_SIZE: usize = 4096;

#[inline]
fn current_input_buffer(mut stream: impl Read) {
    let mut buffer = InputBuffer::with_capacity(CHUNK_SIZE);
    while buffer.read_from(&mut stream).unwrap() != 0 {}
}

#[inline]
fn fast_input_buffer(mut stream: impl Read) {
    let mut buffer = ReadBuffer::<CHUNK_SIZE>::new();
    while buffer.read_from(&mut stream).unwrap() != 0 {}
}

fn benchmark(c: &mut Criterion) {
    const STREAM_SIZE: usize = 1024 * 1024 * 4;
    let data: Vec<u8> = (0..STREAM_SIZE).map(|_| rand::random()).collect();
    let stream = Cursor::new(data);

    let mut group = c.benchmark_group("buffers");
    group.throughput(Throughput::Bytes(STREAM_SIZE as u64));
    group.bench_function("InputBuffer", |b| {
        b.iter(|| current_input_buffer(black_box(stream.clone())))
    });
    group.bench_function("ReadBuffer", |b| b.iter(|| fast_input_buffer(black_box(stream.clone()))));
    group.finish();
}

criterion_group!(benches, benchmark);
criterion_main!(benches);
