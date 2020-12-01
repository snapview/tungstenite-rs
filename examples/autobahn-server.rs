use std::{
    net::{TcpListener, TcpStream},
    thread::spawn,
};

use log::*;
use tungstenite::{
    extensions::compression::{deflate::DeflateConfigBuilder, WsCompression},
    handshake::HandshakeRole,
    protocol::WebSocketConfig,
    server::accept_with_config,
    Error, HandshakeError, Message, Result,
};

fn must_not_block<Role: HandshakeRole>(err: HandshakeError<Role>) -> Error {
    match err {
        HandshakeError::Interrupted(_) => panic!("Bug: blocking socket would block"),
        HandshakeError::Failure(f) => f,
    }
}

fn handle_client(stream: TcpStream) -> Result<()> {
    let deflate_config = DeflateConfigBuilder::default().max_message_size(None).build();

    let mut socket = accept_with_config(
        stream,
        Some(WebSocketConfig {
            compression: WsCompression::Deflate(deflate_config),
            ..Default::default()
        }),
    )
    .map_err(must_not_block)?;
    info!("Running test");
    loop {
        match socket.read_message()? {
            msg @ Message::Text(_) | msg @ Message::Binary(_) => {
                socket.write_message(msg)?;
            }
            Message::Ping(_) | Message::Pong(_) | Message::Close(_) => {}
        }
    }
}

fn main() {
    env_logger::init();

    let server = TcpListener::bind("127.0.0.1:9002").unwrap();

    for stream in server.incoming() {
        spawn(move || match stream {
            Ok(stream) => {
                if let Err(err) = handle_client(stream) {
                    match err {
                        Error::ConnectionClosed | Error::Protocol(_) | Error::Utf8 => (),
                        e => error!("test: {}", e),
                    }
                }
            }
            Err(e) => error!("Error accepting stream: {}", e),
        });
    }
}
