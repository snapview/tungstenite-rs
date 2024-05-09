#![cfg(feature = "handshake")]
use std::{
    net::TcpListener,
    thread::{sleep, spawn},
    time::Duration,
};
use tungstenite::{
    accept_hdr, connect,
    error::{Error, ProtocolError, SubProtocolError},
    handshake::{
        client::generate_key,
        server::{Request, Response},
    },
};

fn create_http_request(uri: &str, subprotocols: Option<Vec<String>>) -> http::Request<()> {
    let uri = uri.parse::<http::Uri>().unwrap();

    let authority = uri.authority().unwrap().as_str();
    let host =
        authority.find('@').map(|idx| authority.split_at(idx + 1).1).unwrap_or_else(|| authority);

    if host.is_empty() {
        panic!("Empty host name");
    }

    let mut builder = http::Request::builder()
        .method("GET")
        .header("Host", host)
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Sec-WebSocket-Version", "13")
        .header("Sec-WebSocket-Key", generate_key());

    if let Some(subprotocols) = subprotocols {
        builder = builder.header("Sec-WebSocket-Protocol", subprotocols.join(","));
    }

    builder.uri(uri).body(()).unwrap()
}

fn server_thread(port: u16, server_subprotocols: Option<Vec<String>>) {
    spawn(move || {
        let server = TcpListener::bind(("127.0.0.1", port))
            .expect("Can't listen, is this port already in use?");

        let callback = |_request: &Request, mut response: Response| {
            if let Some(subprotocols) = server_subprotocols {
                let headers = response.headers_mut();
                headers.append("Sec-WebSocket-Protocol", subprotocols.join(",").parse().unwrap());
            }
            Ok(response)
        };

        let client_handler = server.incoming().next().unwrap();
        let mut client_handler = accept_hdr(client_handler.unwrap(), callback).unwrap();
        client_handler.close(None).unwrap();
    });
}

#[test]
fn test_server_send_no_subprotocol() {
    server_thread(3012, None);
    sleep(Duration::from_secs(1));

    let err =
        connect(create_http_request("ws://127.0.0.1:3012", Some(vec!["my-sub-protocol".into()])))
            .unwrap_err();

    assert!(matches!(
        err,
        Error::Protocol(ProtocolError::SecWebSocketSubProtocolError(
            SubProtocolError::NoSubProtocol
        ))
    ));
}

#[test]
fn test_server_sent_subprotocol_none_requested() {
    server_thread(3013, Some(vec!["my-sub-protocol".to_string()]));
    sleep(Duration::from_secs(1));

    let err = connect(create_http_request("ws://127.0.0.1:3013", None)).unwrap_err();

    assert!(matches!(
        err,
        Error::Protocol(ProtocolError::SecWebSocketSubProtocolError(
            SubProtocolError::ServerSentSubProtocolNoneRequested
        ))
    ));
}

#[test]
fn test_invalid_subprotocol() {
    server_thread(3014, Some(vec!["invalid-sub-protocol".to_string()]));
    sleep(Duration::from_secs(1));

    let err = connect(create_http_request(
        "ws://127.0.0.1:3014",
        Some(vec!["my-sub-protocol".to_string()]),
    ))
    .unwrap_err();

    assert!(matches!(
        err,
        Error::Protocol(ProtocolError::SecWebSocketSubProtocolError(
            SubProtocolError::InvalidSubProtocol
        ))
    ));
}

#[test]
fn test_request_multiple_subprotocols() {
    server_thread(3015, Some(vec!["my-sub-protocol".to_string()]));
    sleep(Duration::from_secs(1));
    let (_, response) = connect(create_http_request(
        "ws://127.0.0.1:3015",
        Some(vec![
            "my-sub-protocol".to_string(),
            "my-sub-protocol-1".to_string(),
            "my-sub-protocol-2".to_string(),
        ]),
    ))
    .unwrap();

    assert_eq!(
        response.headers().get("Sec-WebSocket-Protocol").unwrap(),
        "my-sub-protocol".parse::<http::HeaderValue>().unwrap()
    );
}

#[test]
fn test_request_single_subprotocol() {
    server_thread(3016, Some(vec!["my-sub-protocol".to_string()]));
    sleep(Duration::from_secs(1));

    let (_, response) = connect(create_http_request(
        "ws://127.0.0.1:3016",
        Some(vec!["my-sub-protocol".to_string()]),
    ))
    .unwrap();

    assert_eq!(
        response.headers().get("Sec-WebSocket-Protocol").unwrap(),
        "my-sub-protocol".parse::<http::HeaderValue>().unwrap()
    );
}
