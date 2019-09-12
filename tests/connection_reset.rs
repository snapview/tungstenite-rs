//! Verifies that the server returns a `ConnectionClosed` error when the connection
//! is closedd from the server's point of view and drop the underlying tcp socket.

use std::net::TcpListener;
use std::process::exit;
use std::thread::{sleep, spawn};
use std::time::Duration;

use tungstenite::{accept, connect, Error, Message};
use url::Url;

#[test]
fn test_close() {
    env_logger::init();

    spawn(|| {
        sleep(Duration::from_secs(5));
        println!("Unit test executed too long, perhaps stuck on WOULDBLOCK...");
        exit(1);
    });

    let server = TcpListener::bind("127.0.0.1:3012").unwrap();

    let client_thread = spawn(move || {
        let (mut client, _) = connect(Url::parse("ws://localhost:3012/socket").unwrap()).unwrap();

        client
            .write_message(Message::Text("Hello WebSocket".into()))
            .unwrap();

        let message = client.read_message().unwrap(); // receive close from server
        assert!(message.is_close());

        let err = client.read_message().unwrap_err(); // now we should get ConnectionClosed
        match err {
            Error::ConnectionClosed => {}
            _ => panic!("unexpected error: {:?}", err),
        }
    });

    let client_handler = server.incoming().next().unwrap();
    let mut client_handler = accept(client_handler.unwrap()).unwrap();

    let message = client_handler.read_message().unwrap();
    assert_eq!(message.into_data(), b"Hello WebSocket");

    client_handler.close(None).unwrap(); // send close to client

    assert!(client_handler.read_message().unwrap().is_close()); // receive acknowledgement

    let err = client_handler.read_message().unwrap_err(); // now we should get ConnectionClosed
    match err {
        Error::ConnectionClosed => {}
        _ => panic!("unexpected error: {:?}", err),
    }

    drop(client_handler);

    client_thread.join().unwrap();
}
