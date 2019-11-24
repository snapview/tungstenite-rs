use std::net::TcpListener;
use std::thread::spawn;

use tungstenite::accept_hdr;
use tungstenite::handshake::server::{Request, Response};
use tungstenite::http::StatusCode;

fn main() {
    let server = TcpListener::bind("127.0.0.1:3012").unwrap();
    for stream in server.incoming() {
        spawn(move || {
            let callback = |_req: &Request, _resp| {
                let resp = Response::builder()
                    .status(StatusCode::FORBIDDEN)
                    .body(Some("Access denied".into()))
                    .unwrap();
                Err(resp)
            };
            accept_hdr(stream.unwrap(), callback).unwrap_err();
        });
    }
}
