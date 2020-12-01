use log::*;
use url::Url;

use tungstenite::{
    client::connect_with_config,
    connect,
    extensions::compression::{deflate::DeflateConfigBuilder, WsCompression},
    protocol::WebSocketConfig,
    Error, Message, Result,
};

const AGENT: &str = "Tungstenite";

fn get_case_count() -> Result<u32> {
    let (mut socket, _) = connect(Url::parse("ws://localhost:9001/getCaseCount").unwrap())?;
    let msg = socket.read_message()?;
    socket.close(None)?;
    Ok(msg.into_text()?.parse::<u32>().unwrap())
}

fn update_reports() -> Result<()> {
    let (mut socket, _) = connect(
        Url::parse(&format!("ws://localhost:9001/updateReports?agent={}", AGENT)).unwrap(),
    )?;
    socket.close(None)?;
    Ok(())
}

fn run_test(case: u32) -> Result<()> {
    info!("Running test case {}", case);
    let case_url =
        Url::parse(&format!("ws://localhost:9001/runCase?case={}&agent={}", case, AGENT)).unwrap();
    let deflate_config = DeflateConfigBuilder::default().max_message_size(None).build();

    let (mut socket, _) = connect_with_config(
        case_url,
        Some(WebSocketConfig {
            compression: WsCompression::Deflate(deflate_config),
            ..Default::default()
        }),
        3,
    )?;

    loop {
        match socket.read_message()? {
            msg @ Message::Text(_) | msg @ Message::Binary(_) => {
                socket.write_message(msg)?;
            }
            Message::Ping(_) | Message::Pong(_) | Message::Close(_) => {}
        }
    }
}

fn main() {
    env_logger::init();

    let total = get_case_count().unwrap();

    for case in 1..=total {
        if let Err(e) = run_test(case) {
            match e {
                Error::ConnectionClosed | Error::Protocol(_) | Error::Utf8 => (),
                err => error!("test: {}", err),
            }
        }
    }

    update_reports().unwrap();
}
