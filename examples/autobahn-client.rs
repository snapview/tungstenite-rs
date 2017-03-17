#[macro_use] extern crate log;
extern crate env_logger;
extern crate tungstenite;
extern crate url;

use url::Url;

use tungstenite::{connect, Error, Result};

const AGENT: &'static str = "Tungstenite";

fn get_case_count() -> Result<u32> {
    let mut socket = connect(
        Url::parse("ws://localhost:9001/getCaseCount").unwrap()
    )?;
    let msg = socket.read_message()?;
    socket.close()?;
    Ok(msg.into_text()?.parse::<u32>().unwrap())
}

fn update_reports() -> Result<()> {
    let mut socket = connect(
        Url::parse(&format!("ws://localhost:9001/updateReports?agent={}", AGENT)).unwrap()
    )?;
    socket.close()?;
    Ok(())
}

fn run_test(case: u32) -> Result<()> {
    info!("Running test case {}", case);
    let case_url = Url::parse(
        &format!("ws://localhost:9001/runCase?case={}&agent={}", case, AGENT)
    ).unwrap();
    let mut socket = connect(case_url)?;
    loop {
        let msg = socket.read_message()?;
        socket.write_message(msg)?;
    }
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

