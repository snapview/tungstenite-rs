#![cfg(feature = "handshake")]
#![cfg(feature = "url")]

use std::{
    net::TcpListener,
    process::exit,
    thread::{sleep, spawn},
    time::Duration,
};
use tungstenite::{
    accept_hdr, connect,
    handshake::server::{Request, Response},
    Error, Message,
};

/// Test for write buffering and flushing behaviour.
#[test]
fn test_with_url() {
    env_logger::init();
    let url = url::Url::parse("ws://127.0.0.1:3013/socket").unwrap();

    spawn(|| {
        sleep(Duration::from_secs(5));
        println!("Unit test executed too long, perhaps stuck on WOULDBLOCK...");
        exit(1);
    });

    let server = TcpListener::bind("127.0.0.1:3013").unwrap();

    let client_thread = spawn(move || {
        let (mut client, _) = connect(url).unwrap();
        client.send(Message::Text("Hello WebSocket".into())).unwrap();

        let message = client.read().unwrap(); // receive close from server
        assert!(message.is_close());

        let err = client.read().unwrap_err(); // now we should get ConnectionClosed
        match err {
            Error::ConnectionClosed => {}
            _ => panic!("unexpected error: {:?}", err),
        }
    });

    let callback = |req: &Request, response: Response| {
        println!("Received a new ws handshake");
        println!("The request's path is: {}", req.uri().path());
        Ok(response)
    };

    let client_handler = server.incoming().next().unwrap();
    let mut client_handler = accept_hdr(client_handler.unwrap(), callback).unwrap();

    client_handler.close(None).unwrap(); // send close to client

    // This read should succeed even though we already initiated a close
    let message = client_handler.read().unwrap();
    assert_eq!(message.into_data(), b"Hello WebSocket");

    assert!(client_handler.read().unwrap().is_close()); // receive acknowledgement

    let err = client_handler.read().unwrap_err(); // now we should get ConnectionClosed
    match err {
        Error::ConnectionClosed => {}
        _ => panic!("unexpected error: {:?}", err),
    }

    drop(client_handler);

    client_thread.join().unwrap();
}
