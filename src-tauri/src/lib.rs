use mumble_protocol::mumble_proto as proto;
use mumble_protocol::Message;
use tauri_plugin_log::fern::colors::ColoredLevelConfig;
use tokio::{self, io::AsyncReadExt};

mod AcceptAnyCert;

// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

#[tauri::command]
async fn connect_disconnect(server: String, port: u16) -> Result<(), String> {
    log::info!("Attempting to connect to server {} on port {}", server, port);
    let server_name = rustls::pki_types::ServerName::try_from(server.clone()).map_err(|e| e.to_string())?;
    let tcp = tokio::net::TcpStream::connect((server.clone(), port)).await.map_err(|e| e.to_string())?;
    let tls = AcceptAnyCert::make_connector().map_err(|e| e.to_string())?
        .connect(server_name, tcp);
    let (mut tcp_read, mut tcp_write) = tokio::io::split(tls);
    log::info!("Connected to server {} on port {}", server, port);

    loop {
        let msg_type = tcp_read.read_u16().await.map_err(|e| e.to_string())?;
        let len = tcp_read.read_u32().await.map_err(|e| e.to_string())? as usize;
        let mut payload = vec![0u8; len];
        tcp_read.read_exact(&mut payload).await.map_err(|e| e.to_string())?;

        let message = match msg_type {
            0 => proto::Version::decode(&payload[..]).map_err(|e| e.to_string())?,
            _ => return Err(format!("Unknown message type: {}", msg_type)),
        };
        log::info!("Received message: {:?}", message);

        // Break after first message
        break;
    }
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_log::Builder::new()
            .level(log::LevelFilter::Info)
            .level_for("mumbo_lib", log::LevelFilter::Trace)
            .with_colors(ColoredLevelConfig::default())
            .build())
        .invoke_handler(tauri::generate_handler![greet, connect_disconnect])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
