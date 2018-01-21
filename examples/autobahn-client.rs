extern crate env_logger;
#[macro_use]
extern crate log;
extern crate tungstenite;
extern crate url;

use url::Url;

use tungstenite::{connect, Error, Message, Result};

const AGENT: &'static str = "Tungstenite";

fn get_case_count() -> Result<u32> {
    let (mut socket, _) = connect(Url::parse("ws://localhost:9001/getCaseCount").unwrap())?;
    let msg = socket.read_message()?;
    socket.close(None)?;
    Ok(msg.into_text()?.parse::<u32>().unwrap())
}

fn update_reports() -> Result<()> {
    let (mut socket, _) = connect(
        Url::parse(&format!(
            "ws://localhost:9001/updateReports?agent={}",
            AGENT
        )).unwrap(),
    )?;
    socket.close(None)?;
    Ok(())
}

fn run_test(case: u32) -> Result<()> {
    info!("Running test case {}", case);
    let case_url = Url::parse(&format!(
        "ws://localhost:9001/runCase?case={}&agent={}",
        case, AGENT
    )).unwrap();
    let (mut socket, _) = connect(case_url)?;
    loop {
        match socket.read_message()? {
            msg @ Message::Text(_) | msg @ Message::Binary(_) => {
                socket.write_message(msg)?;
            }
            Message::Ping(_) | Message::Pong(_) => {}
        }
    }
}

fn main() {
    env_logger::init();

    let total = get_case_count().unwrap();

    for case in 1..(total + 1) {
        if let Err(e) = run_test(case) {
            match e {
                Error::Protocol(_) => {}
                err => {
                    warn!("test: {}", err);
                }
            }
        }
    }

    update_reports().unwrap();
}
