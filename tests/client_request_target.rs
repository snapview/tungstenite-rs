#![cfg(feature = "handshake")]
#![allow(clippy::result_large_err)]

use std::{net::TcpListener, thread, time::Duration};
use tungstenite::{
    accept_hdr,
    client::IntoClientRequest,
    connect,
    handshake::{
        client::generate_request,
        server::{Request, Response},
    },
};

#[test]
fn query_without_path_has_a_root_request_target() {
    for scheme in ["ws", "wss"] {
        for (suffix, target) in [
            ("?token=value", "/?token=value"),
            ("?", "/?"),
            ("?a=%2F&b=c+d", "/?a=%2F&b=c+d"),
            ("", "/"),
            ("/?token=value", "/?token=value"),
            ("/path?token=value", "/path?token=value"),
        ] {
            let uri = format!("{scheme}://example.com{suffix}");
            let (request, _) = generate_request(uri.into_client_request().unwrap()).unwrap();
            let line = request.split(|byte| *byte == b'\r').next().unwrap();
            assert_eq!(line, format!("GET {target} HTTP/1.1").as_bytes());
        }
    }
}

#[test]
fn connect_with_query_and_no_path() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        stream.set_write_timeout(Some(Duration::from_secs(5))).unwrap();
        accept_hdr(stream, |request: &Request, response: Response| {
            assert_eq!(request.uri().path(), "/");
            assert_eq!(request.uri().query(), Some("token=value"));
            Ok(response)
        })
        .map(|_| ())
    });

    let result = connect(format!("ws://{address}?token=value"));
    let server_result = server.join().unwrap();
    assert!(result.is_ok(), "client handshake failed: {result:?}");
    assert!(server_result.is_ok(), "server handshake failed: {server_result:?}");
}
