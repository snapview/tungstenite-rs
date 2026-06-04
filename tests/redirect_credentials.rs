#![cfg(feature = "handshake")]
#![allow(clippy::result_large_err)]

use http::Uri;
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread::{sleep, spawn},
    time::Duration,
};
use tungstenite::{
    accept_hdr, connect,
    handshake::server::{Request, Response},
    ClientRequestBuilder,
};

/// A server that redirects the client to a different origin must not cause the
/// originally supplied `Authorization` header to be forwarded to that origin.
#[test]
fn auth_header_dropped_on_cross_origin_redirect() {
    let _ = env_logger::try_init();

    spawn(|| {
        sleep(Duration::from_secs(10));
        eprintln!("Unit test executed too long, perhaps stuck on WOULDBLOCK...");
        std::process::exit(1);
    });

    let redirector = TcpListener::bind("127.0.0.1:0").unwrap();
    let target = TcpListener::bind("127.0.0.1:0").unwrap();
    let redirector_port = redirector.local_addr().unwrap().port();
    let target_port = target.local_addr().unwrap().port();

    // First hop: answer the handshake with a redirect to a different port (origin).
    let redirect_thread = spawn(move || {
        let (mut sock, _) = redirector.accept().unwrap();
        let mut buf = Vec::new();
        let mut tmp = [0u8; 256];
        loop {
            let n = sock.read(&mut tmp).unwrap();
            buf.extend_from_slice(&tmp[..n]);
            if n == 0 || buf.windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
        }
        let resp = format!(
            "HTTP/1.1 302 Found\r\nLocation: ws://127.0.0.1:{target_port}/\r\nContent-Length: 0\r\n\r\n"
        );
        sock.write_all(resp.as_bytes()).unwrap();
        sock.flush().unwrap();
    });

    // Second hop: the redirect target records whether it saw the credential.
    let leaked = Arc::new(AtomicBool::new(false));
    let leaked_server = leaked.clone();
    let target_thread = spawn(move || {
        let (stream, _) = target.accept().unwrap();
        let callback = |req: &Request, response: Response| {
            if req.headers().get("authorization").is_some() {
                leaked_server.store(true, Ordering::SeqCst);
            }
            Ok(response)
        };
        let mut ws = accept_hdr(stream, callback).unwrap();
        let _ = ws.read();
    });

    let uri: Uri = format!("ws://127.0.0.1:{redirector_port}/").parse().unwrap();
    let builder =
        ClientRequestBuilder::new(uri).with_header("Authorization", "Bearer super-secret-token");

    let (client, _) = connect(builder).unwrap();
    drop(client);

    redirect_thread.join().unwrap();
    target_thread.join().unwrap();

    assert!(!leaked.load(Ordering::SeqCst), "Authorization header leaked to redirect target");
}
