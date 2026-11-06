//! End-to-end voice latency checks against a real Mumble server, run as a
//! pair of processes (`sender` and `receiver`) in separate containers.
//!
//! - `sender` / `receiver`: a timestamp in every packet measures the
//!   client -> server -> client hop. See `probe`.
//! - `speech-sender` / `speech-receiver`: real speech through the app's mic
//!   and playback pipelines, measuring transmit, network and playback
//!   latency. See `speech`.

use std::{env, error::Error};

mod common;
mod probe;
mod speech;
mod wav;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    match env::args().nth(1).as_deref() {
        Some("sender") => probe::sender().await,
        Some("receiver") => probe::receiver().await,
        Some("speech-sender") => speech::sender().await,
        Some("speech-receiver") => speech::receiver().await,
        _ => Err("usage: e2e-latency <sender|receiver|speech-sender|speech-receiver>".into()),
    }
}
