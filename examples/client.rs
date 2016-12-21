extern crate ws2;
extern crate url;
extern crate env_logger;

use url::Url;
use ws2::protocol::Message;
use ws2::client::connect;
use ws2::protocol::handshake::Handshake;

fn main() {
    env_logger::init().unwrap();

    let mut socket = connect(Url::parse("ws://localhost:3012/socket").unwrap())
        .expect("Can't connect")
        .handshake_wait()
        .expect("Handshake error");

    socket.write_message(Message::Text("Hello WebSocket".into()));
    loop {
        let msg = socket.read_message().expect("Error reading message");
        println!("Received: {}", msg);
    }
    // socket.close();

}
