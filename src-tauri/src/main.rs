// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    #[cfg(target_os = "linux")]
    work_around_nvidia_explicit_sync();

    mumbo_lib::run()
}

/// WebKitGTK crashes on launch under Wayland with NVIDIA's proprietary driver
/// ("Error 71 (Protocol error) dispatching to Wayland display"): it commits
/// buffers without an explicit-sync acquire point. Disabling explicit sync in
/// the driver avoids it while keeping WebKit's DMABUF renderer.
///
/// Remove once fixed upstream: https://bugs.webkit.org/show_bug.cgi?id=280210
#[cfg(target_os = "linux")]
fn work_around_nvidia_explicit_sync() {
    const VAR: &str = "__NV_DISABLE_EXPLICIT_SYNC";
    // Visible inside flatpak too: /proc is the host's kernel view.
    let nvidia = std::path::Path::new("/proc/driver/nvidia").exists();
    if nvidia && std::env::var_os(VAR).is_none() {
        // SAFETY: called first thing in `main`, before any other threads exist.
        unsafe { std::env::set_var(VAR, "1") };
    }
}
