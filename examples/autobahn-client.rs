use log::*;

use tungstenite::{
    client::connect_with_config, connect, protocol::WebSocketConfig, Error, Message, Result,
};

const AGENT: &str = "Tungstenite";

// Passing `None` to `connect_with_config` is exactly `connect`, so the feature-off build
// negotiates what it always did.
#[cfg(feature = "deflate")]
fn deflate_config() -> Option<WebSocketConfig> {
    Some(WebSocketConfig::default().enable_deflate())
}

#[cfg(not(feature = "deflate"))]
fn deflate_config() -> Option<WebSocketConfig> {
    None
}

fn get_case_count() -> Result<u32> {
    let (mut socket, _) = connect("ws://localhost:9001/getCaseCount")?;
    let msg = socket.read()?;
    socket.close(None)?;
    Ok(msg.into_text()?.as_str().parse::<u32>().unwrap())
}

fn update_reports() -> Result<()> {
    let (mut socket, _) = connect(format!("ws://localhost:9001/updateReports?agent={AGENT}"))?;
    socket.close(None)?;
    Ok(())
}

fn run_test(case: u32) -> Result<()> {
    info!("Running test case {case}");
    let case_url = format!("ws://localhost:9001/runCase?case={case}&agent={AGENT}");
    // Only the case connections carry the extension. The control endpoints stay on the stock
    // `connect` so a negotiation defect cannot masquerade as a lost case count.
    let (mut socket, _) = connect_with_config(case_url, deflate_config(), 3)?;
    loop {
        match socket.read()? {
            msg @ Message::Text(_) | msg @ Message::Binary(_) => {
                socket.send(msg)?;
            }
            Message::Ping(_) | Message::Pong(_) | Message::Close(_) | Message::Frame(_) => {}
        }
    }
}

fn main() {
    env_logger::init();

    let total = get_case_count().unwrap();

    for case in 1..=total {
        if let Err(e) = run_test(case) {
            match e {
                Error::ConnectionClosed | Error::Protocol(_) | Error::Utf8(_) => (),
                err => error!("test: {err}"),
            }
        }
    }

    update_reports().unwrap();
}
