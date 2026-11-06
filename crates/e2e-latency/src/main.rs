//! Measures client -> server -> client voice latency against a real Mumble
//! server. Run one process as `sender` and another as `receiver`; the receiver
//! prints a latency summary and exits non-zero if it exceeds the thresholds.
//!
//! Each voice packet carries its send time (unix nanos) in `opus_data`. The
//! server relays the payload without decoding it, so no codec is needed. This
//! only works when both processes share a clock (same host / same VM), which
//! is the case for containers on one compose network.

use std::{env, error::Error, str::FromStr, time::{Duration, SystemTime, UNIX_EPOCH}};

use mumble_protocol::client::{ConnectionOptions, connect};

const FRAME_INTERVAL: Duration = Duration::from_millis(20);
// Roughly the size of a 20ms Opus voice frame, so the packet size is realistic.
const PAYLOAD_LEN: usize = 80;

fn env_or<T: FromStr>(key: &str, default: T) -> T
where
    T::Err: std::fmt::Debug,
{
    match env::var(key) {
        Ok(v) => v.parse().unwrap_or_else(|e| panic!("bad {key}={v:?}: {e:?}")),
        Err(_) => default,
    }
}

fn now_nanos() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos() as u64
}

async fn connect_with_retry(name: String) -> mumble_protocol::client::Connection {
    // The server takes a few seconds to come up and compose can't tell us when.
    let mut attempts = 0;
    loop {
        let options = ConnectionOptions {
            server: env_or("MUMBLE_SERVER", "localhost".to_string()),
            port: Some(env_or("MUMBLE_PORT", 64738)),
            password: Some(env_or("MUMBLE_PASSWORD", "test".to_string())),
            username: Some(name.clone()),
        };
        match connect(options).await {
            Ok(c) => return c,
            Err(e) if attempts < 30 => {
                attempts += 1;
                eprintln!("connect failed ({e}); retrying ({attempts}/30)");
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            Err(e) => panic!("could not connect: {e}"),
        }
    }
}

async fn sender() -> Result<(), Box<dyn Error>> {
    let connection = connect_with_retry(env_or("PROBE_NAME", "sender".to_string())).await;
    eprintln!("sender connected; sending a frame every {FRAME_INTERVAL:?}");

    // Runs until the receiver finishes and compose tears us down.
    let mut ticker = tokio::time::interval(FRAME_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut frame = 0u64;
    loop {
        ticker.tick().await;
        let mut payload = vec![0u8; PAYLOAD_LEN];
        payload[..8].copy_from_slice(&now_nanos().to_le_bytes());
        connection.client.send_audio(&payload, frame, false)?;
        frame += 1;
    }
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

async fn receiver() -> Result<(), Box<dyn Error>> {
    let wanted: usize = env_or("PROBE_PACKETS", 500);
    let warmup: usize = env_or("PROBE_WARMUP", 25);
    let max_p95_ms: f64 = env_or("MAX_P95_MS", 50.0);
    let max_loss_pct: f64 = env_or("MAX_LOSS_PCT", 1.0);
    let timeout = Duration::from_secs(env_or("PROBE_TIMEOUT_SECS", 90));

    let mut connection = connect_with_retry(env_or("PROBE_NAME", "receiver".to_string())).await;
    eprintln!("receiver connected; waiting for {wanted} packets (+{warmup} warmup)");

    let mut latencies_ms = Vec::with_capacity(wanted);
    let (mut first_frame, mut last_frame) = (None, 0u64);
    let mut seen = 0usize;

    let collect = async {
        while latencies_ms.len() < wanted {
            let Some(audio) = connection.audio.recv().await else {
                return Err("connection closed before enough audio arrived");
            };
            let received = now_nanos();
            let Some(sent) = audio.opus_data.get(..8) else { continue };
            let sent = u64::from_le_bytes(sent.try_into().unwrap());

            seen += 1;
            if seen <= warmup {
                continue;
            }
            first_frame.get_or_insert(audio.frame_number);
            last_frame = audio.frame_number;
            latencies_ms.push(received.saturating_sub(sent) as f64 / 1e6);
        }
        Ok(())
    };
    tokio::time::timeout(timeout, collect).await.map_err(|_| "timed out waiting for audio")??;

    let expected = (last_frame - first_frame.unwrap() + 1) as f64;
    let loss_pct = 100.0 * (1.0 - latencies_ms.len() as f64 / expected);
    latencies_ms.sort_by(|a, b| a.total_cmp(b));
    let mean = latencies_ms.iter().sum::<f64>() / latencies_ms.len() as f64;
    let p95 = percentile(&latencies_ms, 0.95);

    println!(
        "latency_ms: n={} min={:.2} mean={:.2} p50={:.2} p95={:.2} p99={:.2} max={:.2} loss={:.2}%",
        latencies_ms.len(),
        latencies_ms[0],
        mean,
        percentile(&latencies_ms, 0.5),
        p95,
        percentile(&latencies_ms, 0.99),
        latencies_ms.last().unwrap(),
        loss_pct,
    );

    if p95 > max_p95_ms {
        return Err(format!("p95 {p95:.2}ms exceeds limit {max_p95_ms}ms").into());
    }
    if loss_pct > max_loss_pct {
        return Err(format!("loss {loss_pct:.2}% exceeds limit {max_loss_pct}%").into());
    }
    Ok(())
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    match env::args().nth(1).as_deref() {
        Some("sender") => sender().await,
        Some("receiver") => receiver().await,
        _ => Err("usage: e2e-latency <sender|receiver>".into()),
    }
}
