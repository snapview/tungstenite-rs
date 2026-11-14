use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    fmt::{self, Write},
};

use tungstenite::protocol::frame::{
    coding::{Data, OpCode},
    Frame,
};

thread_local! {
    static ALLOCATED: Cell<Option<usize>> = const { Cell::new(None) };
}

struct CountingAllocator;

fn allocated(bytes: usize) {
    let _ = ALLOCATED.try_with(|count| {
        if let Some(prior) = count.get() {
            count.set(Some(prior + bytes));
        }
    });
}

// SAFETY: every allocation is delegated unchanged to the system allocator.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        allocated(layout.size());
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        allocated(size);
        unsafe { System.realloc(ptr, layout, size) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

struct Full;

impl Write for Full {
    fn write_str(&mut self, _: &str) -> fmt::Result {
        Err(fmt::Error)
    }
}

#[test]
fn display_stops_before_allocating_payload_when_writer_refuses() {
    let frame = Frame::message(vec![0xab; 16 * 1024 * 1024], OpCode::Data(Data::Binary), true);
    ALLOCATED.with(|count| count.set(Some(0)));
    let result = write!(Full, "{frame}");
    let bytes = ALLOCATED.with(|count| count.replace(None).unwrap());
    assert!(result.is_err());
    assert_eq!(bytes, 0, "an exhausted formatter must not allocate a payload-sized temporary");
}

#[test]
fn display_preserves_payload_hex_and_layout() {
    let frame = Frame::message(vec![0xab, 0x00, 0xff], OpCode::Data(Data::Binary), true);
    assert_eq!(format!("{frame}"), "\n<FRAME>\nfinal: true\nreserved: false false false\nopcode: BINARY\nlength: 5\npayload length: 3\npayload: 0xab00ff\n            ");
}
