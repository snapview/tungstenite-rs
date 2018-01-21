extern crate env_logger;
#[macro_use]
extern crate log;
extern crate tungstenite;

use std::net::{TcpListener, TcpStream};
use std::thread::spawn;

use tungstenite::{accept, Error, HandshakeError, Message, Result};
use tungstenite::handshake::HandshakeRole;

fn must_not_block<Role: HandshakeRole>(err: HandshakeError<Role>) -> Error {
    match err {
        HandshakeError::Interrupted(_) => panic!("Bug: blocking socket would block"),
        HandshakeError::Failure(f) => f,
    }
}

fn handle_client(stream: TcpStream) -> Result<()> {
    let mut socket = accept(stream).map_err(must_not_block)?;
    loop {
        match socket.read_message()? {
            msg @ Message::Text(_) | msg @ Message::Binary(_) => {
                socket.write_message(msg)?;
            }
            Message::Ping(_) | Message::Pong(_) => {}
        }
    }
}

fn main() {
    env_logger::init();

    let server = TcpListener::bind("127.0.0.1:9001").unwrap();

    for stream in server.incoming() {
        spawn(move || match stream {
            Ok(stream) => match handle_client(stream) {
                Ok(_) => (),
                Err(e) => warn!("Error in client: {}", e),
            },
            Err(e) => warn!("Error accepting stream: {}", e),
        });
    }
}
