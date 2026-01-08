use tungstenite::connect;

fn main() {
    let target = "wss://echo.websocket.events";
    if std::env::var("HTTP_PROXY").is_err()
        && std::env::var("HTTPS_PROXY").is_err()
        && std::env::var("ALL_PROXY").is_err()
    {
        eprintln!("Set HTTP_PROXY, HTTPS_PROXY, or ALL_PROXY before running this example.");
        return;
    }

    let (mut socket, _response) = connect(target).expect("connect through proxy");
    let _ = socket.close(None);
}
