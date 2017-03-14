extern crate tungstenite;
extern crate url;
extern crate env_logger;

use url::Url;
use tungstenite::protocol::Message;
use tungstenite::client::connect;

fn main() {
    env_logger::init().unwrap();

    let mut socket = connect(Url::parse("ws://localhost:3012/socket").unwrap())
        .expect("Can't connect");

    socket.write_message(Message::Text("Hello WebSocket".into())).unwrap();
    loop {
        let msg = socket.read_message().expect("Error reading message");
        println!("Received: {}", msg);
    }
    // socket.close();

}
