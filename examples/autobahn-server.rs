#[macro_use] extern crate log;
extern crate env_logger;
extern crate ws2;

use std::net::{TcpListener, TcpStream};
use std::thread::spawn;

use ws2::server::accept;
use ws2::error::Result;
use ws2::handshake::Handshake;

fn handle_client(stream: TcpStream) -> Result<()> {
    let mut socket = accept(stream).handshake_wait()?;
    loop {
        let msg = socket.read_message()?;
        socket.write_message(msg)?;
    }
}

fn main() {
    env_logger::init().unwrap();

    let server = TcpListener::bind("127.0.0.1:9001").unwrap();

    for stream in server.incoming() {
        spawn(move || {
            match stream {
                Ok(stream) => match handle_client(stream) {
                    Ok(_) => (),
                    Err(e) => warn!("Error in client: {}", e),
                },
                Err(e) => warn!("Error accepting stream: {}", e),
            }
        });
    }
}
