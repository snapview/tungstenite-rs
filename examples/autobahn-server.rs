#[macro_use] extern crate log;
extern crate env_logger;
extern crate tungstenite;

use std::net::{TcpListener, TcpStream};
use std::thread::spawn;

use tungstenite::server::accept;
use tungstenite::handshake::HandshakeError;
use tungstenite::error::{Error, Result};

fn must_not_block<Stream, Role>(err: HandshakeError<Stream, Role>) -> Error {
    match err {
        HandshakeError::Interrupted(_) => panic!("Bug: blocking socket would block"),
        HandshakeError::Failure(f) => f,
    }
}

fn handle_client(stream: TcpStream) -> Result<()> {
    let mut socket = accept(stream).map_err(must_not_block)?;
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
