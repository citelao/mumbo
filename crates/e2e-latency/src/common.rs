use std::{env, str::FromStr, time::{Duration, SystemTime, UNIX_EPOCH}};

use mumble_protocol::client::{Connection, ConnectionOptions, connect};

pub fn env_or<T: FromStr>(key: &str, default: T) -> T
where
    T::Err: std::fmt::Debug,
{
    match env::var(key) {
        Ok(v) => v.parse().unwrap_or_else(|e| panic!("bad {key}={v:?}: {e:?}")),
        Err(_) => default,
    }
}

/// Wall-clock time in unix nanos. Containers on one compose network share a
/// clock, so these are comparable across processes.
pub fn now_nanos() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos() as u64
}

pub async fn connect_with_retry(name: String) -> Connection {
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
