#![no_main]
#[macro_use] extern crate libfuzzer_sys;
extern crate tungstenite;

use std::io::Cursor;

fuzz_target!(|data: &[u8]| {
    let vector: Vec<u8> = data.into();
    let mut cursor = Cursor::new(vector);

    tungstenite::protocol::frame::Frame::parse(&mut cursor);
});
