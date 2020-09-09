use log::*;
use url::Url;

use tungstenite::client::connect_with_config;
use tungstenite::extensions::compression::CompressionConfig;
use tungstenite::protocol::WebSocketConfig;
use tungstenite::{connect, Error, Message, Result};

const AGENT: &str = "Tungstenite";

fn get_case_count() -> Result<u32> {
    let (mut socket, _) = connect_with_config(
        Url::parse("ws://localhost:9001/getCaseCount").unwrap(),
        Some(WebSocketConfig {
            max_send_queue: None,
            max_message_size: Some(64 << 20),
            max_frame_size: Some(16 << 20),
            compression_config: CompressionConfig::deflate(),
        }),
    )?;
    let msg = socket.read_message()?;
    socket.close(None)?;
    Ok(msg.into_text()?.parse::<u32>().unwrap())
}

fn update_reports() -> Result<()> {
    let (mut socket, _) = connect(
        Url::parse(&format!(
            "ws://localhost:9001/updateReports?agent={}",
            AGENT
        ))
        .unwrap(),
    )?;
    socket.close(None)?;
    Ok(())
}

fn run_test(case: u32) -> Result<()> {
    info!("Running test case {}", case);
    let case_url = Url::parse(&format!(
        "ws://localhost:9001/runCase?case={}&agent={}",
        case, AGENT
    ))
    .unwrap();
    let (mut socket, _) = connect_with_config(
        case_url,
        Some(WebSocketConfig {
            max_send_queue: None,
            max_message_size: Some(64 << 20),
            max_frame_size: Some(16 << 20),
            compression_config: CompressionConfig::deflate(),
        }),
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
    println!("Starting");

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
