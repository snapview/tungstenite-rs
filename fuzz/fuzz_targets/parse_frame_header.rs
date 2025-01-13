#![no_main]
#[macro_use] extern crate libfuzzer_sys;
extern crate layer8_tungstenite;

use std::io::Cursor;

fuzz_target!(|data: &[u8]| {
    let vector: Vec<u8> = data.into();
    let mut cursor = Cursor::new(vector);

    layer8_tungstenite::protocol::frame::FrameHeader::parse(&mut cursor).ok();
});
