#![cfg(all(feature = "handshake", not(any(feature = "native-tls", feature = "__rustls-tls"))))]

use tungstenite::{connect, error::UrlError, Error};

#[test]
fn wss_url_fails_when_no_tls_support() {
    let ws = connect("wss://127.0.0.1/ws");
    eprintln!("{:?}", ws);
    assert!(matches!(ws, Err(Error::Url(UrlError::TlsFeatureNotEnabled))))
}
