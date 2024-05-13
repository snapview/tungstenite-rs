#![cfg(feature = "handshake")]
#![cfg(feature = "url")]

use std::{
    assert,
    net::TcpListener,
    println,
    process::exit,
    thread::{sleep, spawn},
    time::Duration,
};
use tungstenite::{
    accept_hdr, connect,
    handshake::server::{Request, Response},
};

/// Test for write buffering and flushing behaviour.
#[test]
fn test_with_url() {
    env_logger::init();
    // notice the use of url::Url instead of a string
    // notice the feature url is activated
    let url = url::Url::parse("ws://127.0.0.1:3013").unwrap();

    spawn(|| {
        sleep(Duration::from_secs(5));
        println!("Unit test executed too long, perhaps stuck on WOULDBLOCK...");
        exit(1);
    });

    let server = TcpListener::bind("127.0.0.1:3013").unwrap();

    let client_thread = spawn(move || {
        let conn = connect(url);
        assert!(conn.is_ok());
    });

    let client_handler = server.incoming().next().unwrap();

    let closing =
        accept_hdr(client_handler.unwrap(), |_: &Request, r: Response| Ok(r)).unwrap().close(None);
    assert!(closing.is_ok());

    let result = client_thread.join();
    assert!(result.is_ok());
}
