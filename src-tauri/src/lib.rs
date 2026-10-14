use mumble_protocol::mumble_proto as proto;
use tauri_plugin_log::fern::colors::ColoredLevelConfig;
use tokio;

// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

#[tauri::command]
async fn connect_disconnect(server: String, port: u16) -> Result<(), String> {
    log::info!("Attempting to connect to server {} on port {}", server, port);
    let tcp = tokio::net::TcpStream::connect((server, port)).await.map_err(|e| e.to_string())?;
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
