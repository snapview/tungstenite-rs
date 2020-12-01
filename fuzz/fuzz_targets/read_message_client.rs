#![no_main]
#[macro_use]
extern crate libfuzzer_sys;
extern crate tungstenite;

use std::io;
use std::io::Cursor;
use tungstenite::extensions::uncompressed::UncompressedExt;
use tungstenite::protocol::Role;
use tungstenite::WebSocket;
//use std::result::Result;

// FIXME: copypasted from tungstenite's protocol/mod.rs

struct WriteMoc<Stream>(Stream);

impl<Stream> io::Write for WriteMoc<Stream> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<Stream: io::Read> io::Read for WriteMoc<Stream> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf)
    }
}

// end of copypasta

fuzz_target!(|data: &[u8]| {
    //let vector: Vec<u8> = data.into();
    let cursor = Cursor::new(data);
    let mut socket = WebSocket::from_raw_socket(WriteMoc(cursor), Role::Client, None);
    socket.read_message().ok();
});
