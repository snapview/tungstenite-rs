#[macro_use] extern crate log;
extern crate env_logger;
extern crate ws2;
extern crate url;

use url::Url;

use ws2::protocol::Message;
use ws2::client::connect;
use ws2::handshake::Handshake;
use ws2::error::{Error, Result};

const AGENT: &'static str = "WS2-RS";

fn get_case_count() -> Result<u32> {
    let mut socket = connect(
        Url::parse("ws://localhost:9001/getCaseCount").unwrap()
    )?.handshake_wait()?;
    let msg = socket.read_message()?;
    socket.close();
    Ok(msg.into_text()?.parse::<u32>().unwrap())
}

fn update_reports() -> Result<()> {
    let mut socket = connect(
        Url::parse(&format!("ws://localhost:9001/updateReports?agent={}", AGENT)).unwrap()
    )?.handshake_wait()?;
    socket.close();
    Ok(())
}

fn run_test(case: u32) -> Result<()> {
    info!("Running test case {}", case);
    let case_url = Url::parse(
        &format!("ws://localhost:9001/runCase?case={}&agent={}", case, AGENT)
    ).unwrap();
    let mut socket = connect(case_url)?.handshake_wait()?;
    loop {
        let msg = socket.read_message()?;
        socket.write_message(msg)?;
    }
    socket.close();
    Ok(())
}

fn main() {
    env_logger::init().unwrap();

    let total = get_case_count().unwrap();

    for case in 1..(total + 1) {
        if let Err(e) = run_test(case) {
            match e {
                Error::Protocol(_) => { }
                err => { warn!("test: {}", err); }
            }
        }
    }

    update_reports().unwrap();
}

