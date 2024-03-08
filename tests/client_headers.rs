#![cfg(feature = "handshake")]

use http::Uri;
use std::{
    net::TcpListener,
    process::exit,
    thread::{sleep, spawn},
    time::Duration,
};
use tungstenite::{
    accept_hdr, connect,
    handshake::server::{Request, Response},
    ClientRequestBuilder, Error, Message,
};

/// Test for write buffering and flushing behaviour.
#[test]
fn test_headers() {
    env_logger::init();
    let uri: Uri = "ws://127.0.0.1:3013/socket".parse().unwrap();
    let token = "my_jwt_token";
    let full_token = format!("Bearer {token}");
    let sub_protocol = "my_sub_protocol";
    let builder = ClientRequestBuilder::new(uri)
        .with_header("Authorization", full_token.to_owned())
        .with_sub_protocol(sub_protocol.to_owned());

    spawn(|| {
        sleep(Duration::from_secs(5));
        println!("Unit test executed too long, perhaps stuck on WOULDBLOCK...");
        exit(1);
    });

    let server = TcpListener::bind("127.0.0.1:3013").unwrap();

    let client_thread = spawn(move || {
        let (mut client, _) = connect(builder).unwrap();
        client.send(Message::Text("Hello WebSocket".into())).unwrap();

        let message = client.read().unwrap(); // receive close from server
        assert!(message.is_close());

        let err = client.read().unwrap_err(); // now we should get ConnectionClosed
        match err {
            Error::ConnectionClosed => {}
            _ => panic!("unexpected error: {:?}", err),
        }
    });

    let callback = |req: &Request, mut response: Response| {
        println!("Received a new ws handshake");
        println!("The request's path is: {}", req.uri().path());
        println!("The request's headers are:");
        let authorization_header: String = "authorization".to_ascii_lowercase();
        let web_socket_proto: String = "sec-websocket-protocol".to_ascii_lowercase();

        for (ref header, value) in req.headers() {
            println!("* {}: {}", header, value.to_str().unwrap());
            if header.to_string() == authorization_header {
                println!("Matching authorization header");
                assert_eq!(header.to_string(), authorization_header);
                assert_eq!(value.to_str().unwrap(), full_token);
            } else if header.to_string() == web_socket_proto {
                println!("Matching sec-websocket-protocol header");
                assert_eq!(header.to_string(), web_socket_proto);
                assert_eq!(value.to_str().unwrap(), sub_protocol);
                // the server needs to respond with the same sub-protocol
                response
                    .headers_mut()
                    .append("sec-websocket-protocol", sub_protocol.parse().unwrap());
            }
        }
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
